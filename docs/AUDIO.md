# Audio : niveaux réels (B7) et mixeur par app (E9) — `crates/nox-audio`

Machine : MacBook Air M2, macOS 27.0 (26A428). Dernière mise à jour : 2026-10-25.
Propriétaire : Agent Mesures/Audio (branche `agent/audio`).

## Principe et vie privée

Les deux fonctions reposent sur les **process taps Core Audio** (`CATapDescription` +
`AudioHardwareCreateProcessTap`, macOS 14.2+ ; Nox exige 14.4 comme la référence) lus
via un **agrégat privé** sur la sortie par défaut et un **IOProc** temps réel.

* Lire l'audio d'une app exige l'autorisation **« Enregistrement audio du système »**
  (TCC). macOS l'affiche **à la première création d'un tap**, avec le texte de
  `NSAudioCaptureUsageDescription` (Info.plist).
* Opt-in strict, désactivé par défaut :
  * B7 : `MusicSettings.live_levels` (false). Un tap n'existe que si
    `live_levels && lecture en cours && section Musique visible (MusicCommand::Visible)`.
  * E9 : module `Mixer` désactivé par défaut (`settings.modules`). Même activé, **une app
    à 100 % et non muette n'a aucun tap** ; un tap n'est créé que pour une app réglée.
* Aucun audio n'est stocké ni envoyé. B7 garde ≈ 0,17 s d'échantillons mono en mémoire.
  E9 ne garde rien : chaque bloc est rendu puis oublié.
* Taps et agrégats sont **privés** (`isPrivate`) : invisibles des autres apps, et macOS
  les détruit si Nox quitte ou plante.
* Il n'existe pas d'API publique pour lire l'état TCC. Un tap refusé est quand même créé
  mais ne livre que du silence ; Nox interprète **8 s de silence numérique** comme
  « autorisation probablement refusée » (B7 : `MeterEvent::Silent` ; E9 :
  `MixerState.permission = Denied`, l'app est rendue à la normale pour la session).
* Signature ad hoc : l'identité du code change à chaque reconstruction, donc macOS
  redemande l'autorisation après chaque nouvelle build (seul Developer ID règle ça).

## Chaîne B7 (`LevelMeter`)

1. Processus du lecteur : `kAudioHardwarePropertyProcessObjectList`, puis pour chaque objet
   `kAudioProcessPropertyPID`, rattaché au lecteur si le pid est le sien ou si son
   « app propriétaire » l'est (`responsibility_get_pid_responsible_for_pid`, puis la
   chaîne des parents jusqu'à une app régulière : les helpers Chrome/Opera et
   `com.apple.WebKit.GPU` → Safari/Mail). Vérifié ici (`--example audio_probe`).
2. Tap stéréo **non muet** (`CATapUnmuted`), donc le son de l'app est inchangé ; puis
   agrégat privé {sous-périphérique = sortie par défaut, tap, drift compensation,
   auto-start}.
3. IOProc : met les sorties à zéro, repère le buffer du tap (le dernier dont le nombre de
   canaux est celui du tap, jamais une supposition parmi plusieurs, pour ne pas prendre le
   micro) et le copie, mixé en mono, dans un anneau **sans verrou** (atomiques). Pas
   d'allocation, pas de log.
4. Fil `nox-audio-meter` à 30 Hz : 1024 derniers échantillons, fenêtre de Hann, FFT
   radix-2 maison, 7 bandes (50-120-250-500-1k-2k-4,5k-12 kHz) en dB avec un plancher à
   −60 dB, puis lissage (montée rapide, descente lente, échelle auto).
5. Chaque seconde : si la sortie par défaut change, l'agrégat est reconstruit ; si les
   processus changent, le tap est reconstruit (nouveaux processus) ou le meter s'arrête
   avec `ProcessesLeft` (un processus tapé a disparu).

Intégration (agent Intégrations, module musique) :

```rust
let meter = nox_audio::LevelMeter::start(player_pid, Default::default(), move |e| match e {
    nox_audio::MeterEvent::Levels(l) => hub.update(|s| if let Some(m) = &mut s.music {
        m.levels = l.to_vec(); m.levels_measured = true;
    }),
    // Silent / Unavailable / ProcessesLeft : revenir à l'animation
    // (levels vide, levels_measured = false) ; ProcessesLeft : relancer si
    // toujours en lecture ; Silent : ne pas relancer pour ce morceau.
    _ => { /* … */ }
})?;
// Pause > ~1,5 s, section masquée, live_levels désactivé : drop(meter).
```

## Chaîne E9 (module `Mixer`)

* Lignes = apps régulières qui tiennent une connexion audio, avec leurs helpers
  regroupés (même résolution qu'en B7). Les démons et Nox lui-même sont exclus. Les apps
  qui jouent sont listées en premier. `id` = bundle id, sinon `pid:<n>` (session seulement,
  rien sur disque).
* Volume `0..=2` (1 = inchangé), muet. Priorité : commande `MixerCommand` reçue pendant la
  session > `settings.mixer` (par bundle id uniquement) > inchangé. C'est l'UI qui persiste
  (`SettingsStore`) ; le module applique les commandes en direct.
* App réglée : tap **muet** (`CATapMuted`, le son d'origine est retiré de la sortie), agrégat
  privé sur la sortie par défaut, IOProc qui re-rend le tap avec le gain (rampe linéaire sur
  le bloc, sans cliquetis ; limiteur doux au-delà de 1, genou à 0,8, jamais > ±1 ; mono →
  deux canaux avant ; canaux en trop silencieux).
* Retour à 100 % et non muet : moteur détruit et son d'origine rendu.
  `ResetAll` : toutes les apps à 100 %.
* Chaque seconde (visible ou moteurs actifs, sinon toutes les 4 s) : liste rafraîchie,
  moteurs reconstruits si la sortie par défaut ou les processus de l'app changent.
* Fail-open : un tap silencieux pendant 8 s alors que l'app déclare jouer
  (`kAudioProcessPropertyIsRunningOutput`) est retiré et l'app marquée « refusée » pour la
  session (jusqu'à ce que l'utilisateur la règle à nouveau).

## Mesures exécutées sur ce M2 (sans aucun tap)

| Vérification | Résultat |
|---|---|
| `cargo test -p nox-audio` | 19 tests unitaires OK : FFT comparée à une DFT naïve, sinus pleine échelle ≈ 0 dB dans sa bande, silence = 0, plages de bandes, anneau (mixdown, NaN, latest), lissage, propriétaire via responsabilité/parents, `tap_change`, choix du buffer du tap, rendu (mapping, rampe, mute, limiteur, offsets par buffer), regroupement et cibles du mixeur, commandes |
| `audio_probe` (aucune autorisation) | 39 clients audio listés ; helpers Chrome/Opera/Claude/ChatGPT → leur app, `com.apple.WebKit.GPU` → Mail, démons sans propriétaire |
| Agrégat privé + IOProc **sans tap** sur « Haut-parleurs MacBook Air » (0 entrée : le test refuse une sortie qui a des entrées, pour ne jamais ouvrir le micro) | 1875 cycles d'E/S en 20 s (512 trames à 48 kHz, cohérent) ; **0,16 % d'un cœur** en régime établi (création exclue) |
| Analyse (FFT + bandes + lissage, release) | **17 µs par mise à jour**, 0,05 % d'un cœur à 30 Hz |
| Module mixeur, toutes les apps à 100 % | 9 apps publiées, `engaged = false` partout (aucun tap), `mixer = None` après stop |

Coût estimé de B7 en marche : ≈ 0,2 % d'un cœur côté Nox (IOProc + analyse), plus le
travail de coreaudiod pour le tap, non mesuré.

## À valider par l'utilisateur (crée un tap → invite système)

Tant que ces étapes n'ont pas été exécutées, B7 et E9 sont « implémentées », pas « testées ».

1. B7 : lancer Musique, puis
   `NOX_ALLOW_TAP=1 NOX_TAP_PID=$(pgrep -x Music) cargo test -p nox-audio --release --test hardware -- --ignored --nocapture live_tap`
   → accepter l'invite → des niveaux s'affichent et suivent la musique. Refuser l'invite →
   `Silent` au bout d'environ 8 s.
2. E9 : activer le module Mixeur, baisser une app qui joue à 30 % → le son baisse ; muet →
   silence ; 200 % → plus fort sans saturation dure ; retour à 100 % → `engaged = false`.
3. Fail-open : avec une app à 30 %, `kill -9` de Nox → le son de l'app revient à son niveau
   normal immédiatement.
4. Changer de sortie (casque Bluetooth) pendant un réglage : le réglage suit en environ 1 s.

## Limites connues

* La sortie utilisée est la sortie par défaut du système. Une app qui sort sur un autre
  périphérique (choix propre à l'app) est re-rendue sur la sortie par défaut.
* Sur une sortie à plus de 2 canaux, la mixdown du tap divise le niveau (FB13479345, compensé
  dans la référence) : non compensé ici. B7 s'auto-échelonne ; pour E9, le niveau reste plus
  faible sur ces sorties.
* Une app « en lecture » qui n'émet que du silence numérique pendant 8 s avec un volume
  réglé est prise pour un refus : elle revient à la normale (fail-open) jusqu'au prochain
  réglage.
* Latence : re-rendre via un agrégat ajoute environ un cycle d'E/S (≈ 10 ms à 512
  trames/48 kHz) au son des apps réglées.
