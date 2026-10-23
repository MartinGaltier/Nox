# Organisation du développement (équipe d'agents)

Nox est développé par un coordinateur et cinq agents Claude Code travaillant en parallèle,
chacun dans son propre worktree Git et sa propre branche :

| Agent | Branche | Périmètre |
|---|---|---|
| Interface | `agent/interface` | `nox-ui` : îlot, animations, réglages, barre des menus, moniteur |
| Intégrations | `agent/integrations` | musique, calendrier, notifications, caméra, accessoires, téléchargements, fichiers, raccourcis |
| Activités | `agent/activities` | agents IA, surveillance de zone, minuteurs, compagnon |
| Mesures | `agent/metrics` | capteurs, statistiques, historique |
| Énergie | `agent/power` | veille, capot fermé, helper privilégié, récupération |

Mécanisme : sous-agents nommés lancés en arrière-plan par le coordinateur (outil `Agent`),
communiquant par messages (`SendMessage`) entre eux et avec le coordinateur. Il ne s'agit pas
de la fonctionnalité expérimentale « Agent Teams » à équipes multiples, mais d'une équipe
implicite unique de la session.

Règles : contrats communs dans `nox-core` (propriété du coordinateur), propriété de fichiers
par agent (`docs/ARCHITECTURE.md`), intégration sur `main` uniquement par le coordinateur
(merge sans réécriture d'historique), revues croisées après intégration, statut de parité
par agent dans `docs/parity/<agent>.md` fusionné dans `docs/PARITY.md`.
