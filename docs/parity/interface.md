# Parité — Interface (A1–A20, sections, fenêtres)

Référence : Vorssaint `e916961` (voir docs/REFERENCE.md). Statuts : **à faire** · **en cours** ·
**implémentée** (code + tests automatiques) · **testée** (vérifiée sur le MacBook Air M2 réel, preuve notée) ·
**bloquée** (raison).

## Architecture de `nox-ui`

* **Couche pure (testée sans AppKit)** : `geometry` (encoche, tailles, silhouette, hit-test),
  `spring` (ressort), `machine` (compact/déplié, survol + délais), `alerts` (file à priorité + TTL),
  `compact` (activité prioritaire), `gesture` (balayage/défilement), `island` (modèle complet),
  `views/*` (chaque section produit une `Scene` : opérations de dessin + zones cliquables),
  `format` (français, valeurs `Reading` honnêtes).
* **Couche AppKit (`src/appkit`)** : panneau non activant, peintre (NSBezierPath, NSString, SF Symbols,
  pochettes), menu de barre, fenêtres Réglages et Moniteur, overlay de sélection de zone, raccourci global
  Carbon, ouverture à la connexion (SMAppService), hôte de calque caméra.
* **Zéro réveil au repos** : le minuteur d’animation (120 Hz) n’existe que tant qu’un ressort n’est pas posé ;
  les autres minuteurs sont des one-shot (délai de survol/fermeture, TTL d’alerte, prochaine seconde d’un
  compte à rebours visible). Aucun minuteur quand l’îlot est compact sans activité.
* **Données d’aperçu** : `NOX_UI_FIXTURE=1` (builds debug uniquement, module `#[cfg(debug_assertions)]`),
  ne remplit que les emplacements qu’aucun module ne publie, chaque texte est marqué « [aperçu] ».
  `NOX_UI_FORCE=expanded:<section>|compact` (debug uniquement) force l’état pour les captures.

## Lignes A

| ID | Fonction | Implémentation Nox | Vérification automatique | Statut |
|---|---|---|---|---|
| A1 | Fenêtre îlot dans l’encoche | `NSPanel` borderless + `NonactivatingPanel`, niveau `NSStatusWindowLevel + 1`, `canJoinAllSpaces | stationary | fullScreenAuxiliary | ignoresCycle`, fond transparent, silhouette noire (épaules + coins, adaptée de `NotchShape`), ombre uniquement déplié ; géométrie via `safeAreaInsets` + `auxiliaryTopLeft/RightArea` | `geometry::tests::notched_display_centres_on_camera` (M2 : 185×32 centré, collé au bord haut) | implémentée — capture réelle en attente (disque plein, voir Limites) |
| A2 | États compact / déplié | `machine::Machine` + `island::IslandModel::decide` (repos, bande d’activité, alerte, page) | `machine::tests::*`, `island::tests::hover_expands_*` | implémentée |
| A3 | Survol, délai, clic | Zone de suivi limitée à la forme (`island_contains`), `hover_delay_ms`, `close_delay_ms`, clic = ouvrir ; réglages appliqués à chaud (`Machine::configure`) | `machine::tests::*` | implémentée |
| A4 | Animations | Ressort Rust (`spring::IslandMotion`), minuteur actif seulement pendant l’animation ; fondu contenu compact ↔ page | `spring::tests::*`, `island::tests::rest_is_exactly_the_camera…` | implémentée — CPU au repos à mesurer |
| A5 | Aucun vol de focus | `canBecomeKeyWindow`/`canBecomeMainWindow` = NO, panneau non activant, `acceptsFirstMouse` | — | implémentée — test physique à faire |
| A6 | Écrans sans encoche | Capsule dans la barre ou encoche suspendue (`IslandSettings.silhouette`), rien au repos si `simulate_notch` = false ; choix d’écran `DisplayChoice` | `geometry::tests::external_display_simulates_or_hides` | implémentée |
| A7 | Changement d’écran | `NSApplicationDidChangeScreenParametersNotification` → nouvelle géométrie, sans animation entre écrans | — | implémentée |
| A8 | Plein écran | Liste des fenêtres de l’app au premier plan (sans titres : aucune permission) ; masqué si une fenêtre couvre tout l’écran de l’îlot ; recalcul sur changement d’espace / d’app active (pas de sondage) | `screens::tests::coverage_rules` | implémentée |
| A9 | Sections activables / réorganisables | Réglages › Sections : case « afficher », case « module actif » (arrêt réel via `on_settings_changed`), ▲▼ | `views::tests`, `island::tests::section_paging_skips_disabled_modules` | implémentée |
| A10 | Gestes | Balayage horizontal = section suivante/précédente (adapté de `NotchGestureSupport`), tirer vers le bas = ouvrir, pousser = fermer, défilement vertical dans les listes, haptique `NSHapticFeedbackManager` | `gesture::tests::*`, `island::tests::list_scroll_is_clamped` | implémentée |
| A11 | Alertes temporaires | `hub.drain_alerts` → `AlertQueue` (priorité, préemption, TTL), notice sous l’encoche, clic = ouvrir la section liée | `alerts::tests::*`, `island::tests::alert_shows_notice…` | implémentée |
| A12 | Barre des menus | `NSStatusItem` : ouvrir l’îlot, état + temps restant du maintien éveillé, modes × durées (15/30/60/120/∞), arrêt, assistant capot fermé, moniteur, réglages, quitter (`on_quit` puis `terminate:`) ; menu reconstruit à chaque ouverture | — | implémentée |
| A13 | Réglages | Fenêtre AppKit native (onglets Général, Sections, Contenus, Agents & surveillance, Système & énergie) ; `store.save` puis `on_settings_changed` | — | implémentée |
| A14 | Arrêt des collectes à la désactivation | Case « module actif » → `ModuleManager` | test core `disabling_a_module_stops_it` | implémentée (core) |
| A15 | Masqué dans les captures | `sharingType = none` si `hide_in_captures` (et pendant une surveillance de zone) | — | implémentée |
| A16 | Masquer jusqu’au survol | `hide_until_hover` : rien n’est dessiné au repos tant que le pointeur n’est pas sur l’îlot | — | implémentée |
| A17 | Taille / silhouette / contour / translucide | `size` (compacte 440 / spacieuse 520 / personnalisée 360–640), `silhouette`, `outline`, `translucent` | `geometry::tests::expanded_size_respects_presets_and_screen` | implémentée |
| A18 | Écran verrouillé | Nécessite une API privée SkyLight pour une fenêtre au-dessus de l’écran de verrouillage | — | bloquée (API privée non retenue) |
| A19 | Raccourci global | ⌃⌥N via Carbon `RegisterEventHotKey` (aucune permission Accessibilité), option `shortcut_enabled` | — | implémentée |
| A20 | Contenu au repos | `idle_content` : rien / batterie / musique en pause / agents | `compact::tests::*` | implémentée |

## Rendu des sections (îlot déplié)

| Section | Contenu | Absence de données |
|---|---|---|
| Musique | pochette, titre/artiste/album, lecteur (clic = ouvrir), progression interpolée (`elapsed_now`) + seek, précédent/lecture/suivant, aléatoire/répétition si exposés, paroles synchronisées (ligne active) ou file, égaliseur (marqué « non mesuré » si animé) | module absent / « Rien en lecture » / raison des paroles ou de la file |
| Calendrier | événements N jours, groupés par jour, horaire, lieu, « dans 10 min », Rejoindre (visio), clic = ouvrir | autorisation (bouton Autoriser ou Réglages Système) / « Aucun événement » |
| Agents IA | carte par fournisseur détecté : sessions (phase, projet, outil, modèle), limites + reset, jetons du jour, coût « ≈ … $ (estimé) » | « Aucun agent détecté », limites désactivées (opt-in) |
| Minuteurs | minuteurs (pause/reprise/+1 min/annuler, préréglages), chronomètre (tours), Pomodoro | état vide par colonne |
| Notifications | app, titre, corps (masquable), heure relative, effacer, tout effacer | autorisation Accessibilité / vide |
| Téléchargements | progression (ou indéterminée), octets, débit, temps restant (estimé), source, afficher/ouvrir/retirer | vide |
| Fichiers | étagère (cible de glisser-déposer → `FileCommand::Add`), AirDrop, compresser, copier les chemins, vider, ajouter… | zone de dépôt |
| Système | CPU, GPU, mémoire + pression, batterie, réseau, puissance + mini-graphes à trous ; `MetricsVisible(true/false)` selon la visibilité | « Indisponible » / raison, jamais 0 |
| Maintien éveillé | état + temps restant, 3 modes × 5 durées, arrêt, assistant capot fermé (explication puis `InstallHelper`) | module absent |
| Accessoires | appareils connectés, batteries G/D/boîtier | vide / raison |
| Surveillance | phase, cible, taille, dernier changement, choix du déclencheur → overlay de sélection → `WatchCommand::Start` | autorisation Enregistrement d’écran |
| Caméra | hôte de calque pour l’aperçu d’`integrations`, Start/Stop selon visibilité, choix de caméra | autorisation / « Aperçu en attente du module caméra » tant que le hook n’existe pas |
| Raccourcis | actions rapides des réglages + raccourcis Raccourcis.app, dernier résultat | vide |

Fenêtre **Moniteur système** : cœurs P/E, GPU, capteurs, mémoire/pression/swap, batterie (santé, cycles,
autonomie, flux), puissance, réseau (↓/↑ + historique), disques, ventilateurs — historiques bornés avec trous.

## Coordination

* Caméra : hook `set_preview_host(Option<usize>)` à fournir par `integrations` dans `model/camera.rs` ;
  côté UI `appkit/camera.rs` (calque hôte retenu par l’UI, main thread uniquement).
* Surveillance : overlay côté UI ; `BeginSelection` → overlay → `Start{region, trigger}` / `Stop`
  (passera à `CancelSelection` quand le modèle d’`activities` sera sur main).
* Modèles annoncés mais pas encore sur main (adaptation à l’intégration) : `PowerState` (Toggle,
  `helper_busy`, `lid_sleep_disabled: Reading<bool>`, `ClientRejected`), `MusicState` (`unavailable`,
  `automation`, `RequestAutomation`, `lyrics_online`), `CompanionState` (intensity, reaction…), `WatchState`.

## Icône

`packaging/AppIcon.icns` est originale : générée par `crates/nox-ui/assets/make_icon.py`
(`python3 -I make_icon.py icon_1024.png`, puis `sips -z` aux tailles d’un `.iconset` et `iconutil -c icns`).

## Limites

* A18 (écran verrouillé) non implémenté : demande une API privée.
* La détection plein écran assimile une fenêtre « zoomée » sous une barre des menus masquée à un plein écran.
* Mises à jour d’équaliseur animé à 10 Hz pendant la lecture uniquement (pas de niveaux réels : aucune
  permission d’enregistrement audio demandée).
