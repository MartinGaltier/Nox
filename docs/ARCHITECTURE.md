# Architecture de Nox

## Vue d'ensemble

```
            ┌──────────────── main thread (AppKit) ────────────────┐
            │ nox-ui : NSPanel îlot · rendu CoreGraphics/CALayer    │
            │          menu barre · fenêtre Réglages · animations   │
            └───────▲──────────────────────────────┬────────────────┘
       wake (coalescé, dispatch main)      Command (hub.send)
            │                                      ▼
   ┌────────┴──────────── nox-core::Hub ──────────────────────────┐
   │ IslandState (snapshots par module) · file d'alertes            │
   └────────▲───────────────────────────────────────┬─────────────┘
            │ hub.update / push_alert                │ ModuleManager (thread dédié)
   ┌────────┴──────────┬──────────────┬──────────────┴─┬──────────────┐
   │ nox-integrations  │ nox-activities│ nox-metrics    │ nox-power    │
   │ musique, agenda,  │ agents IA,    │ CPU/GPU/temp/  │ assertions,  │
   │ notifs, caméra,   │ surveillance, │ RAM/batterie/  │ capot fermé ─┼──▶ nox-helper
   │ accessoires, DL,  │ minuteurs,    │ réseau/disque  │ (client)     │   (launchd root,
   │ fichiers, raccourcis│ compagnon   │                │              │    socket Unix)
   └───────────────────┴──────────────┴────────────────┴──────────────┘
```

* `nox-core` : **contrats communs** (aucune dépendance AppKit sauf `dispatch2`).
  * `module::Module` — `start(ctx)`, `stop()`, `command()`, `settings_changed()`.
    Après `stop()`, plus aucun thread, timer, observateur ou handle actif.
  * `hub::Hub` — état partagé (`IslandState`), alertes, réveil UI coalescé, envoi de commandes.
  * `manager::ModuleManager` — démarre/arrête les modules selon `Settings::module_enabled`.
  * `cancel::{CancelToken, Worker}` — threads possédés, annulables, joints à l'arrêt.
  * `reading::Reading<T>` — `Measured` / `Estimated` / `Unavailable(raison)`. **Jamais de 0 à la place d'une erreur.**
  * `model::*` — un fichier par domaine (état, commandes, réglages).
  * `settings` — `Settings` JSON (`~/Library/Application Support/Nox/settings.json`), écriture atomique 0600.
  * `paths`, `permissions`, `main_thread` (`run_async`, `run_sync`).
* `nox-app` : point d'entrée ; assemble modules + hub + UI (propriété du coordinateur).
* `nox-helper` : binaire privilégié minimal (voir docs/SLEEP_RECOVERY.md).

## Fils d'exécution

* AppKit, `NSAppleScript`, `AVCaptureVideoPreviewLayer`, `NSWorkspace` UI → **main thread** via
  `nox_core::main_thread`.
* Chaque module possède ses `Worker`s ; la collecte mutualisée des mesures se fait dans un
  seul worker `nox-metrics` à fréquence réglable (premier plan / arrière-plan via
  `Command::MetricsVisible`).
* Aucun processus externe relancé en boucle. Les seules passerelles non-Rust admises,
  justifiées dans le code et ici :
  * `/usr/bin/perl` hébergeant une cdylib Rust pour MediaRemote (entitlement réservé
    aux binaires plateforme depuis macOS 15.4) — **un seul processus long**, pas de boucle.
  * `/usr/bin/pmset` exécuté **par le helper root** uniquement à l'activation/restauration.
  * `osascript … with administrator privileges` **une seule fois** pour installer le helper,
    après consentement explicite (mot de passe saisi dans la boîte de dialogue système,
    jamais stocké, sudoers jamais modifié).

## Répartition des fichiers (propriété)

| Zone | Propriétaire | Branche |
|---|---|---|
| `crates/nox-core/src/{lib,hub,module,manager,command,alert,settings,cancel,reading,paths,permissions,main_thread}.rs` | Coordinateur | `main` |
| `crates/nox-core/src/model/{music,calendar,notifications,accessories,downloads,files,shortcuts,camera}.rs`, `crates/nox-integrations/**`, `crates/nox-nowplaying/**` | Agent Intégrations | `agent/integrations` |
| `crates/nox-core/src/model/{agents,watch,timers,companion}.rs`, `crates/nox-activities/**` | Agent Activités | `agent/activities` |
| `crates/nox-core/src/model/metrics.rs`, `crates/nox-metrics/**`, `docs/METRICS_SOURCES.md` | Agent Mesures | `agent/metrics` |
| `crates/nox-core/src/model/power.rs`, `crates/nox-power/**`, `crates/nox-helper/**`, `docs/SLEEP_RECOVERY.md`, `scripts/*helper*` | Agent Énergie | `agent/power` |
| `crates/nox-ui/**`, `packaging/AppIcon.icns` | Agent Interface | `agent/interface` |
| `crates/nox-app/**`, `Cargo.toml` racine, `scripts/build-app.sh`, `packaging/*.plist`, CI, `docs/PARITY.md`, README | Coordinateur | `main` |

Un agent qui a besoin d'un changement hors de sa zone (nouvelle variante de `Command`,
champ partagé, dépendance du workspace, entrée Info.plist) l'envoie au coordinateur, ou le
fait dans un commit séparé `chore(core): …` clairement identifié que le coordinateur arbitre
à l'intégration.

## unsafe

Tout `unsafe` est isolé dans des modules `ffi`/`sys` par crate, chaque bloc porte un
commentaire `// SAFETY:` (lint `clippy::undocumented_unsafe_blocks`), et les fonctions
publiques exposées au reste du code sont sûres.

## Données privées

Rien de privé n'est persisté ni committé, à deux exceptions voulues par l'utilisateur : le bloc-notes (`scratchpad/note.txt`, 0600) et le cache des paroles (plafonné à 200 entrées / 90 jours, supprimé quand les paroles sont désactivées). Sinon : notifications et événements restent en mémoire ;
les journaux des agents IA sont lus en place ; aucune clé/cookie n'est copiée. Les jetons OAuth
déjà stockés par les CLI ne sont lus que si `agents.allow_limit_queries` est activé.
