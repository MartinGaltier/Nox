# Parité — Activités (D1–D11)

Statuts : **à faire** · **en cours** · **implémentée** (code + vérification automatique) ·
**testée** (validée sur le Mac M2 réel, preuve notée) · **bloquée** (raison).
Tests : `cargo test -p nox-activities` (55 tests, fixtures synthétiques écrites pour ces tests —
aucun extrait de vrais journaux). Vérifications réelles : exemples `agents_dump`,
`watch_probe`, `activities_smoke` (sorties = agrégats uniquement). Machine : MacBook Air M2,
macOS 27.0 (26A428), 2026-10-24.

| ID | Fonction | Implémentation Nox | Provenance des valeurs | Preuve | Statut |
|---|---|---|---|---|---|
| D1 | Minuteurs multiples, pause/reprise/+temps, renommer, son | `timers/engine.rs` (échéances monotones), `timers/mod.rs` (worker endormi jusqu'à l'échéance, 1 Hz seulement si un minuteur tourne, `NSSound` « Glass » sur le main thread), état dans `Application Support/Nox/timers/state.json` (ancres horloge murale) | Mesuré (horloge) | Tests `countdown_rings_once_at_deadline`, `pause_resume_and_add_time`, `wall_clock_jump_does_not_affect_countdown`, `survives_restart`, `module_rings_restores_and_stops` ; smoke réel : minuteur 1 s → `TimerDone` | implémentée (son non écouté par l'agent) |
| D2 | Chronomètre avec tours | idem (200 tours max) | Mesuré | Test `stopwatch_laps`, restauration après redémarrage | implémentée |
| D3 | Pomodoro (durées, pause longue toutes les N séances, cycles, enchaînement auto ou manuel) | idem, réglages `focus_*` ; une phase échue pendant que Nox était fermé attend l'utilisateur | Mesuré | Tests `pomodoro_cycle`, `manual_advance_waits_when_auto_advance_is_off` | implémentée |
| D4 | Agents — Claude Code | `agents/claude.rs` (JSONL `~/.claude/projects/**`, sous-agents inclus, dédup `message.id`+`requestId` au max), phase **mesurée** via `~/.claude/sessions/<pid>.json` (`status` busy/idle/waiting, `waitingFor`, pid vérifié par `kill(pid,0)`), activité = dernier outil, projet = dossier, modèle ; limites : historique de l'app Claude `plan-usage-history.json` (pourcentages mesurés, réinitialisations **estimées**) ou endpoint OAuth `api/oauth/usage` **seulement si** `agents.allow_limit_queries` | Jetons : mesurés ; coût : estimé ; limites : mesurées (app) / mesurées (OAuth) ; reset : estimé (app) | `agents_dump` réel : 23 fichiers, 1ᵉʳ passage 0,81 s, passage incrémental 0,1 ms ; jetons du jour 185 037 848 = recalcul Python indépendant au jeton près ; limite « Semaine » 11 % (source ClaudeApp, reset estimé) ; forfait « Pro » lu dans `~/.claude.json` (champs organisation uniquement). OAuth : implémenté, **non exécuté** (opt-in désactivé). | testée (lecteur local) ; OAuth implémentée non testée |
| D5 | Agents — Codex | `agents/codex.rs` (rollouts `$CODEX_HOME/sessions/**`, `archived_sessions`), `token_usage_record` prioritaire sinon deltas `token_count` ; `rate_limits` primaire/secondaire + `resets_at` lus localement sans réseau ; forfait (`plan_type`), crédits ; comptes supplémentaires `~/.codex-*` et `agents.codex_homes` | Jetons/limites : mesurés ; coût : estimé | `agents_dump` réel : jetons 30 j 750 652 = recalcul Python ; limites 5 h (réinitialisée depuis la lecture → inconnue, pas 0) et Semaine 1 % ; forfait Plus. Comptes multiples : testés sur fixtures uniquement (pas de 2ᵉ home ici). Réinitialisations « bancaires » : **indisponibles** (seul `codex app-server` les connaît ; Nox ne lance pas de processus) | testée (limites, jetons) ; resets bancaires bloquée (raison ci-contre) |
| D6 | Agents — OpenCode | `agents/opencode.rs` : stockage JSON `~/.local/share/opencode/storage/message/**` et base `opencode.db` (SQLite système, lecture seule, champs extraits en SQL : jamais les prompts) | Jetons : mesurés ; coût : rapporté par OpenCode sinon estimé | Tests `turn_and_usage`, `legacy_file_keeps_only_needed_fields`, `sqlite_database_is_read_incrementally`. OpenCode non installé sur ce Mac. | implémentée |
| D7 | Agents — GitHub Copilot | `agents/copilot.rs` : `~/.copilot/session-state/<id>/events.jsonl`, tours, outils, compteurs cumulés de `session.shutdown` | Jetons : mesurés à l'arrêt de session seulement ; limites : indisponibles (non exposées localement) | Test `turns_tools_and_shutdown_metrics`. Copilot non installé sur ce Mac. | implémentée |
| D8 | Alertes fin / attente / limite | `AgentFinished` si tour terminé ≥ `finish_alert_min_secs` (30 s), `AgentNeedsInput` (registre `waiting`, `AskUserQuestion`, `ExitPlanMode`), `AgentLimit` au franchissement de `limit_threshold_percent` (80 %) une fois par fenêtre ; l'historique lu au démarrage ne déclenche rien | — | Tests `end_to_end_on_synthetic_home` (fin, attente, limite 70→85 %), `history_does_not_alert`, `alert_rules` | implémentée |
| D9 | Coût estimé / budget / période | Table embarquée `data/agent-prices.json` (Vorssaint, revue 2026-10-09 ; tarifs long contexte, cache 1 h, fast, US-only, recherche web) ; `Reading::Estimated` partout ; budget quotidien (`daily_budget_usd`, alerte une fois/jour) ; période Aujourd'hui/7 j/30 j ; réponses sans prix comptées à part | Estimé | Tests `costs`, `family_matching`, `dedup_merge_and_periods` ; `agents_dump` : valeur API du jour affichée « estimate » | implémentée |
| D10 | Surveillance de zone | `watch/` : overlay de sélection côté UI (`BeginSelection` → `Selecting` → `Start{region,trigger}` / `CancelSelection`), capture `SCScreenshotManager captureImageInRect:` (ScreenCaptureKit, macOS 15.2+), vignettes 32×32 gris, déclencheurs « changement » (confirmé `change_confirm_secs`) et « fin d'activité » (`settle_secs`), son « Ping », alerte `WatchTriggered` ; module désactivé par défaut, permission demandée seulement à la sélection, capture arrêtée à la fin | Mesuré (pixels) | Tests tracker + module (déclenchement, refus de permission sans capture, échecs → `Lost`, arrêt immédiat) ; `watch_probe` réel : permission accordée, capture 400×200 en 121 ms, diff 2 % (barre des menus) | implémentée (vérif capture réelle faite ; parcours complet avec l'overlay UI à valider) |
| D11 | Compagnon | `companion.rs` : humeur dérivée de `hub.read` — batterie faible (sur batterie, ≤ seuil métriques) → inquiet ; fin d'agent / minuteur → célébration 20 s ; agent en attente / au travail → occupé ; musique → danse ; Pomodoro → occupé ; inactivité (`CGEventSourceSecondsSinceLastEventType`, sans permission) → somnolent ; réactions ponctuelles (id) ; publication seulement sur changement | Dérivé | Tests `moods_by_priority`, `finishing_celebrates_once_then_calms_down` ; smoke réel : minuteur → `Celebrating/Timer` | implémentée (rendu animé : `nox-ui`) |

## Vie privée

* Journaux des agents lus en place ; seuls compteurs, modèles, heures, noms de dossier et noms
  d'outils en sortent ; rien n'est persisté par le module agents (mémoire bornée : 31 jours,
  400 000 réponses max).
* `~/.claude.json` : seuls `organizationUuid`, `organizationType`, `organizationRateLimitTier`
  sont désérialisés.
* Jeton OAuth Claude (trousseau `Claude Code-credentials`) lu uniquement si
  `agents.allow_limit_queries`, pour la requête puis libéré ; jamais copié, persisté, rafraîchi
  ni journalisé.
* Surveillance : seule une vignette 32×32 gris est conservée en mémoire ; aucune image écrite.

## Limites connues

* Phase « attente » Codex/OpenCode/Copilot : leurs journaux n'enregistrent pas les demandes
  d'approbation ; seules les questions explicites sont détectées. Claude : mesurée par le registre.
* Réinitialisations Claude issues de l'historique de l'app : estimées (marquées `resets_estimated`).
* Sondage léger (2 s si un agent travaille, 5 s sinon) des seuls fichiers récents ; découverte
  toutes les 30 s ; pas de FSEvents.
* Après `shutdown` du smoke test : un thread de plus qu'au départ (probablement GCD), aucun
  worker `nox-*` restant vérifié via `Worker::stop` (joints).
