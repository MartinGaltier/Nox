# Nox

Dynamic Island, moniteur système et maintien éveillé (y compris capot fermé) pour macOS
sur Apple Silicon. Écrit en Rust, interface AppKit native via `objc2`, sans WebView.

Nox reproduit le comportement de [Vorssaint](https://vorssaint.com) sur ce périmètre
(référence épinglée : `docs/REFERENCE.md`). **Nox n'est pas Vorssaint** et n'est ni un
produit officiel ni un produit approuvé par Vorssaint. Licence GPL-3.0-or-later ; voir
`NOTICE.md` pour les attributions.

L'état réel de chaque fonction (à faire / implémentée / testée / bloquée) est dans
**`docs/PARITY.md`**. Ce README ne prétend rien de plus.

## Prérequis

- Mac Apple Silicon, macOS 14 ou plus récent (développé et vérifié sur MacBook Air M2, macOS 27.0).
- Pour compiler : Rust stable (`rustup`), outils en ligne de commande Xcode (`xcode-select --install`).

## Construire

```sh
scripts/build-app.sh                  # release arm64 → dist/Nox.app (signature ad hoc)
NOX_PROFILE=debug scripts/build-app.sh
NOX_SIGN_IDENTITY="Developer ID Application: …" scripts/build-app.sh   # si vous avez une identité
```

Le script est reproductible depuis un clone propre (`--locked`, `SOURCE_DATE_EPOCH` = date
du dernier commit). Contenu du bundle :

| Chemin | Rôle |
|---|---|
| `Contents/MacOS/nox` | l'application |
| `Contents/Library/LaunchServices/nox-helper` | helper root optionnel (capot fermé) |
| `Contents/Resources/libnox_nowplaying.dylib` + `now-playing.pl` | passerelle Now Playing (voir plus bas) |

**Signature** : aucune identité Developer ID n'est disponible sur la machine de
développement. La build est donc **signée ad hoc et non notarisée**. Conséquences :
Gatekeeper refuse le premier lancement d'une copie téléchargée (clic droit → Ouvrir, ou
`xattr -dr com.apple.quarantine Nox.app`), et macOS peut redemander les autorisations
après chaque recompilation (l'empreinte du binaire change).

## Installer

```sh
cp -R dist/Nox.app /Applications/
open /Applications/Nox.app
```

Nox est une app d'arrière-plan (pas d'icône dans le Dock) : une icône apparaît dans la
barre des menus, et l'îlot autour de l'encoche (ou une capsule simulée sur un écran sans encoche).

## Autorisations

Nox ne demande une autorisation qu'au moment où vous activez la fonction qui en a besoin.
Les modules qui en exigent une intrusive sont **désactivés par défaut**.

| Fonction | Autorisation macOS | Par défaut |
|---|---|---|
| Calendrier | Calendriers (accès complet) | activé, demandé au premier usage |
| Notifications dans l'îlot | Accessibilité | **désactivé** |
| Aperçu caméra | Caméra (uniquement quand la page Caméra est ouverte) | **désactivé** |
| Surveillance de zone | Enregistrement de l'écran | **désactivé** |
| Commandes lecture aléatoire / file (Musique, Spotify), vider la corbeille | Automatisation (Apple Events) vers l'app concernée | à la première action |
| Accessoires Bluetooth | Bluetooth | activé |
| Suivi des téléchargements | Dossier Téléchargements | activé |
| Maintien éveillé capot fermé | mot de passe administrateur **une seule fois** pour installer le helper | jamais sans clic explicite |
| Limites Claude via OAuth | aucune ; lit le jeton déjà stocké par Claude Code | **désactivé** (opt-in dans Réglages → Agents IA) |

Lecture en cours : macOS réserve MediaRemote aux binaires de la plateforme depuis 15.4.
Comme Vorssaint, Nox charge sa bibliothèque Rust dans `/usr/bin/perl` (un seul processus
long, environnement vidé, arrêté avec le module). C'est la seule passerelle non Rust
permanente.

## Utilisation

- **Îlot** : survol → déplié (délai réglable), clic sur une section, balayage horizontal au
  trackpad pour changer de section. Il ne prend jamais le focus clavier de l'app active.
- **Sections** : Musique, Calendrier, Agents IA, Minuteurs, Notifications, Téléchargements,
  Fichiers, Système, Maintien éveillé, Accessoires, Surveillance, Caméra, Raccourcis.
  Activables, désactivables et réorganisables dans Réglages → Îlot. Désactiver une section
  ou un module **arrête réellement** sa collecte.
- **Moniteur système** : barre des menus → Moniteur système. Une valeur introuvable
  s'affiche « Indisponible » (jamais 0) ; une valeur calculée porte la mention « estimé ».
  Sources et vérifications : `docs/METRICS_SOURCES.md`.
- **Maintien éveillé** (îlot ou barre des menus) :
  1. *Mac actif* (l'écran peut s'éteindre) ;
  2. *Mac et écran actifs* ;
  3. *Mac actif capot fermé*, même sans écran externe (nécessite le helper).

  Durées 15 min, 30 min, 1 h, 2 h ou illimitée ; option « uniquement sur secteur » ; arrêt
  sous un seuil de batterie (le helper impose de toute façon un plancher de 5 %).
- **Agents IA** : Claude Code, Codex, OpenCode et GitHub Copilot sont lus depuis leurs
  journaux locaux (rien n'est copié ni envoyé). Les tokens sont mesurés et le coût est une
  estimation (table de prix datée).

## Données et vie privée

- Réglages : `~/Library/Application Support/Nox/settings.json` (0600).
- Caches (pochettes, paroles plafonnées à 200 entrées, supprimées si les paroles sont désactivées) : `~/Library/Caches/io.github.martingaltier.nox/`.
- Journal : `~/Library/Logs/Nox/nox.log`. Il ne contient ni titres, ni notifications,
  ni événements, ni prompts.
- Aucune télémétrie. Requêtes réseau : uniquement LRCLIB (paroles, si activées en ligne) et
  l'API d'usage Claude (si l'opt-in OAuth est activé).

## Récupération des réglages de veille

Si le Mac ne se met plus en veille capot fermé après un crash ou un arrêt forcé, le helper
restaure normalement l'état tout seul (fin de bail, redémarrage). Manuellement :

```sh
pmset -g | grep SleepDisabled          # aucune ligne ou 0 = veille normale
sudo pmset -a disablesleep 0           # rétablir la veille capot fermé
sudo scripts/uninstall-helper.sh       # retirer le helper (restaure d'abord si besoin)
```

Procédure complète, modèle de menace et tests physiques à faire : **`docs/SLEEP_RECOVERY.md`**.

## Désinstaller

1. Barre des menus → Réglages → Maintien éveillé → *Désinstaller l'assistant* (ou
   `sudo scripts/uninstall-helper.sh`) si vous l'aviez installé.
2. Quitter Nox, puis :

```sh
rm -rf /Applications/Nox.app \
  ~/Library/Application\ Support/Nox \
  ~/Library/Caches/io.github.martingaltier.nox \
  ~/Library/Logs/Nox
```

3. Facultatif : retirer Nox des listes d'autorisations dans Réglages Système → Confidentialité
   et sécurité (`tccutil reset All io.github.martingaltier.nox`).

## Développement

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                  # tests automatisables (CI)
cargo test --workspace -- --ignored     # tests matériels / permissions, sur un vrai Mac
cargo run -p nox-metrics --example dump          # mesures réelles
cargo run -p nox-activities --example agents_dump # agrégats agents IA
```

Architecture et répartition du code : `docs/ARCHITECTURE.md`. Organisation de l'équipe
d'agents qui a développé le projet : `docs/TEAM.md`. Résultats de tests et limites connues :
`docs/TESTING.md`.
