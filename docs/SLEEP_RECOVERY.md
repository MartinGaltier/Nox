# Maintien éveillé capot fermé : fonctionnement, récupération, limites

Ce document couvre le mode **capot fermé** (`KeepAwakeMode::ClosedLid`). Les deux autres
modes (`System`, `SystemAndDisplay`) n'utilisent que des assertions IOPM détenues par le
processus Nox : le noyau les libère dès que le processus disparaît, il n'y a rien à
récupérer pour eux.

> **Statut :** implémenté et testé automatiquement (logique, journal, gardien, script
> d'installation sur une copie hors `/etc`, lecture de `SleepDisabled`). **Non testé
> physiquement** : l'agent n'a ni installé la règle, ni lancé `sudo`, ni modifié `pmset`.
> Voir § 8 pour la procédure à suivre.

## 1. Pourquoi une règle sudo, et pourquoi plus de daemon root

* Les assertions IOPM (`PreventUserIdleSystemSleep`, …) n'empêchent que la veille
  **d'inactivité**. À la fermeture du capot d'un MacBook sans écran externe, le noyau met
  le Mac en veille quand même.
* Seul `pmset -a disablesleep 1` (`SleepDisabled`) l'empêche, et seul root peut le régler.
* L'ancienne version installait un daemon launchd root (`nox-helper`) qui gardait une
  connexion ouverte avec l'app. Elle est remplacée : **Nox n'a plus aucun processus root.**
  Une règle sudoers étroite suffit à régler `disablesleep`, et la restauration se fait
  par des processus de l'utilisateur (journal, gardien, LaunchAgent).

Méthode retenue, comme Vorssaint (`SudoersSupport.swift`, commit e916961) :

```
#501 ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0
```

* La règle est accordée par **uid numérique** (`#501`), pas par nom : un nom d'utilisateur
  devrait être nettoyé contre la syntaxe sudoers et le shell, un uid ne contient que des
  chiffres.
* Le fichier est `/etc/sudoers.d/nox-lid-sleep`. **Pas de point dans le nom** : sudo ignore
  les fichiers de `/etc/sudoers.d` dont le nom contient un `.` (sudoers(5)). Un fichier
  `io.github.martingaltier.nox` serait lu par personne et la règle n'aurait jamais
  d'effet.
* Nox n'appelle jamais `sudo` sans `-n` : `sudo -n /usr/bin/pmset -a disablesleep 1|0`
  échoue au lieu de demander un mot de passe. **Nox ne voit ni ne stocke aucun mot de
  passe.**

## 2. Compatibilité vérifiée sur ce Mac (macOS 27.0, MacBook Air M2)

Commandes lancées par l'agent, sans privilèges :

```
$ ls -l /usr/bin/pmset
-rwxr-xr-x  1 root  wheel  565168  3 sept. 12:34 /usr/bin/pmset
$ strings /usr/bin/pmset | grep -n "disablesleep\|SleepDisabled"
177:SleepDisabled
178: SleepDisabled
575:disablesleep
$ pmset -g | grep SleepDisabled          # aucune ligne : la valeur est 0
$ ioreg -r -n IOPMrootDomain -d1 -l | grep SleepDisabled
      "SleepDisabled" = No
$ sudo -n -l /usr/bin/pmset -a disablesleep 1; echo $?    # sans règle : 1
```

* `pmset` connaît toujours l'argument `disablesleep`.
* Nox lit la valeur **sans processus externe** : propriété `SleepDisabled` de l'entrée
  `IOPMrootDomain` (`nox_power::probe::sleep_disabled`). Elle vaut `No` sur ce Mac, ce qui
  concorde avec `pmset -g`.
* Je n'ai pas pu vérifier, faute de root, que la propriété passe à `Yes` après
  `pmset -a disablesleep 1`. C'est pourquoi chaque écriture est **confirmée** : Nox relit
  la valeur pendant 3 s et signale une erreur si elle ne change pas.
* La valeur est **persistante** (stockée par powerd). Elle survit au redémarrage : c'est
  pour cela qu'il existe un LaunchAgent (§ 4).

### Ce que `disablesleep` désactive, et ce qu'il ne désactive pas

Lu dans le source xnu (`IOPMrootDomain::checkSystemSleepAllowed`, xnu-12377.121.6) : le
test `userDisabledAllSleep` (posé par `SleepDisabled`) arrive **avant** les exceptions
« batterie faible » et « urgence thermique ». Donc, tant que `SleepDisabled` vaut 1 :

* les veilles d'inactivité, à la fermeture du capot et demandées (menu Pomme) sont refusées ;
* **la veille d'urgence logicielle sur batterie faible et la veille d'urgence thermique
  sont aussi refusées.**

Restent actifs, car hors de cette fonction : la limitation thermique du CPU/GPU et
l'extinction matérielle de protection. Nox ne touche **à aucun réglage thermique**.

Comme `disablesleep` neutralise ces deux protections logicielles, **Nox les remplace** par
des gardes qui tournent même si l'app est figée (gardien, § 3) :

| Garde | Déclencheur | Action |
|---|---|---|
| Batterie (réglage utilisateur) | sur batterie **et** (`only_on_ac`, ou charge ≤ `stop_below_percent`) | session arrêtée, veille rétablie |
| Batterie (plancher absolu) | sur batterie **et** charge ≤ **5 %**, même si le seuil est à 0 | idem |
| Thermique | `com.apple.system.thermalpressurelevel` ≥ 2 (*heavy*) | idem |

Nox évalue ses gardes à chaque tick (au plus 15 s). Le gardien les évalue toutes les 10 s.

## 3. Architecture

```
Nox.app (utilisateur)                                  gardien (utilisateur, même binaire)
  Engine ── LidClient ─┬─ journal  lid-state.json ◀──┐  nox --lid-guard <pid de Nox>
                       ├─ sudo -n pmset …            │   kqueue NOTE_EXIT sur Nox
                       └─ lance le gardien ──────────┼─▶ Nox mort ? → restaure, efface, quitte
                                                     │   Nox vivant + batterie/thermique ?
LaunchAgent (utilisateur, à l'ouverture de session)  │     → restaure, marque le journal, quitte
  nox --lid-restore ─────────────────────────────────┘
```

Fichiers :

* **Règle :** `/etc/sudoers.d/nox-lid-sleep` (root:wheel 0440), créée par
  `scripts/install-lid-rule.sh install <uid> <remove-legacy>`.
* **Journal :** `~/Library/Application Support/Nox/power/lid-state.json` (0600). Il contient
  `active`, `initial` (valeur avant Nox), `owner_pid`, `guard_pid`, `since`, les paramètres
  batterie, et `guard_reason` quand le gardien a restauré seul.
* **LaunchAgent :** `~/Library/LaunchAgents/io.github.martingaltier.nox.lid-restore.plist`
  (sans `UserName`, sans `LaunchDaemons`), lance `nox --lid-restore` à l'ouverture de session.
  Créé avec la règle, retiré avec elle.

### Séquence d'activation (`LidClient::acquire`)

1. La règle est présente (`sudo -n -l`) ; sinon erreur claire, rien n'est modifié.
2. Lecture de `SleepDisabled`. **Déjà à 1 : Nox ne touche à rien** (pas de journal, pas de
   gardien, valeur laissée à l'utilisateur).
3. Journal écrit **avant** toute modification : `active = true`, `initial = false`.
4. Gardien lancé (`process_group(0)`, il survit à un Ctrl-C ou à un arrêt de groupe).
5. `sudo -n pmset -a disablesleep 1`, puis relecture confirmée pendant 3 s. En cas d'échec,
   Nox remet la valeur d'origine et efface le journal.
6. Assertion `PreventUserIdleSystemSleep` comme avant.

### Restauration (`guard::restore`)

* Le journal dit que Nox a posé `1` **et** la valeur lue est encore `1` → `sudo -n pmset … 0`,
  puis confirmation.
* La valeur lue est `0` (changée par l'utilisateur ou déjà restaurée) → **rien n'est
  écrit**. Une valeur choisie par l'utilisateur n'est jamais écrasée.
* Lecture impossible → `0` (le côté sûr). Si l'écriture échoue, le journal reste : le
  gardien réessaie toutes les 10 s (au plus 60 fois), et le prochain lancement aussi.

### Le gardien (`nox --lid-guard <pid>`)

* Attend la fin de Nox par kqueue (`EVFILT_PROC`, `NOTE_EXIT`), avec un délai de 10 s pour
  évaluer les gardes batterie et thermique.
* Nox mort : restaure selon le journal, efface le journal, quitte.
* Gardes déclenchées alors que Nox vit : restaure, marque le journal
  (`active = false`, `guard_reason`), quitte. Nox lit cette marque à son prochain tick,
  arrête la session avec la raison (« Batterie sous le seuil », « Température élevée »)
  et prévient l'utilisateur.
* Sans journal, ou journal d'un autre propriétaire : quitte sans rien faire.
* Nox l'arrête à la fin normale de session (après avoir restauré). Un `SIGTERM` reçu ne
  restaure rien : le journal décide, et il a déjà été effacé.

## 4. Comportement après incident

| Incident | Ce qui se passe |
|---|---|
| Arrêt de session / Quitter | `Engine::stop` → restauration par le journal, arrêt du gardien. |
| Crash de Nox, `kill -9`, arrêt forcé | Le gardien voit Nox disparaître (kqueue) et restaure dans la seconde. Au lancement suivant, `startup_recovery` lit le journal et le trace dans une notice. |
| Nox figé (vivant mais inactif) | Pas de restauration par le gardien : il ne surveille que la mort et les gardes. Les gardes batterie et thermique restent actives. |
| Mac débranché, batterie basse | Gardes de Nox et du gardien, ≤ 10 s. |
| Crash, puis redémarrage (ou coupure) | `SleepDisabled` persiste. À l'ouverture de session, le LaunchAgent lance `nox --lid-restore`, qui lit le journal et restaure. |
| Journal illisible | Traité comme « Nox a posé 1 » : restauration à `0`. |
| Règle retirée pendant une session | La restauration échoue (sudo refuse) : le journal reste, le gardien réessaie, Nox affiche l'erreur. Voir § 5. |

## 5. Installation et retrait

* **Installation :** seulement sur action explicite (le bouton « Autoriser le capot fermé »
  de Maintien éveillé). Une alerte explique ce qui va être installé, puis macOS demande le
  mot de passe administrateur **une fois**. Le script `scripts/install-lid-rule.sh`, embarqué
  dans le binaire, génère lui-même la règle à partir de l'uid numérique, la valide avec
  `visudo -cf` **avant** de l'installer, puis la déplace atomiquement. Rien n'est installé si
  la validation échoue. Ensuite, Nox écrit le LaunchAgent et vérifie `sudo -n -l`.
* **Pourquoi le script ne lit aucun fichier produit par l'utilisateur :** un fichier temporaire
  écrit par Nox, lu par root après l'invite, pourrait être remplacé entre les deux étapes.
  La règle est donc générée en root, à partir d'un nombre seulement.
* **Ancien assistant :** si `/Library/LaunchDaemons/io.github.martingaltier.nox.helper.plist`
  est présent, la même invite le retire (`bootout`, restauration via son `--recover` s'il
  reste un journal, suppression des fichiers).
* **Retrait :** bouton « Retirer l’autorisation capot fermé » (une invite), ou
  `sudo scripts/install-lid-rule.sh remove` depuis le dépôt. Nox arrête d'abord la session
  en cours, donc la veille est restaurée, puis le script supprime `/etc/sudoers.d/nox-lid-sleep`
  et l'ancien helper s'il existe, et Nox supprime le LaunchAgent.

## 6. Menaces et limites

* **Tout processus de cet utilisateur peut basculer `disablesleep`.** La règle autorise
  `sudo -n /usr/bin/pmset -a disablesleep 1|0` à l'uid de l'utilisateur, sans mot de passe.
  C'est le prix de la méthode, comme pour Vorssaint. Un programme malveillant de la session
  peut donc empêcher la veille capot fermé, mais il ne peut rien faire d'autre avec cette
  règle : aucune autre commande, aucun autre fichier.
* **Le gardien est un processus de l'utilisateur.** Un processus du même uid peut le tuer.
  Dans ce cas, Nox restaure à la fin de session, et le LaunchAgent restaure à l'ouverture
  de session suivante. Un `SIGKILL` du gardien pendant que Nox vit ne change rien tant
  que Nox tourne (il restaure à la fin de session). Si Nox meurt ensuite, la veille reste
  désactivée jusqu'au prochain lancement de Nox ou à la prochaine ouverture de session.
* **Un `1` posé par l'utilisateur pendant une session de Nox** est indiscernable de celui de
  Nox : il sera ramené à `0` à la fin de session. Un `0` posé pendant la session est
  respecté.
* **Le binaire de Nox est modifiable par un administrateur.** Le LaunchAgent et le gardien
  pointent vers `Nox.app/Contents/MacOS/nox`. Déplacer l'app après l'installation casse le
  LaunchAgent (launchd journalise l'erreur) : réinstaller l'autorisation depuis la nouvelle
  copie.
* **Pas de signature Developer ID** (ad hoc). Rien ne prouve l'origine du binaire autorisé ;
  seule la règle, elle, est limitée par construction.
* **Sommeil forcé :** pendant une session, `SleepDisabled` bloque aussi la veille demandée
  depuis le menu Pomme. C'est le comportement de macOS.
* **Chaleur :** la garde thermique (*heavy*) arrête la session. C'est volontaire, car
  `disablesleep` désactive la veille d'urgence thermique.

## 7. Récupération manuelle

```sh
# 1. État actuel (aucune ligne = veille normale)
pmset -g | grep SleepDisabled
ioreg -r -n IOPMrootDomain -d1 -l | grep SleepDisabled

# 2. Rétablir la veille capot fermé
sudo pmset -a disablesleep 0

# 3. Retirer la règle, l'ancien helper s'il existe, et le LaunchAgent
sudo scripts/install-lid-rule.sh remove          # depuis le dépôt
rm -f ~/Library/LaunchAgents/io.github.martingaltier.nox.lid-restore.plist
rm -f "$HOME/Library/Application Support/Nox/power/lid-state.json"   # journal, si présent
```

Sans le dépôt, `sudo rm -f /etc/sudoers.d/nox-lid-sleep` suffit pour la règle.

## 8. À tester physiquement (par l'utilisateur)

Rien de ce qui touche `pmset` n'a été exécuté par l'agent. À faire sur le Mac réel, sur
secteur, sans écran externe :

1. Construire `dist/Nox.app` (`scripts/build-app.sh`), l'installer dans `/Applications`,
   ouvrir Nox, choisir « Mac actif capot fermé » puis « Autoriser » et saisir le mot de passe.
   Vérifier : `sudo -n -l /usr/bin/pmset -a disablesleep 1` retourne 0, et
   `cat /etc/sudoers.d/nox-lid-sleep` affiche la ligne `#<uid> ALL=(root) …`.
   Vérifier que `~/Library/LaunchAgents/io.github.martingaltier.nox.lid-restore.plist` existe.
2. Démarrer une session capot fermé de 15 min. Pendant ce temps, `pmset -g | grep SleepDisabled`
   doit afficher `1`, et `ioreg -r -n IOPMrootDomain -d1 -l | grep SleepDisabled` doit afficher
   `Yes`. Fermer le capot 2 min, depuis un autre appareil un `ping` doit passer.
3. Arrêter la session : `pmset -g | grep SleepDisabled` ne doit rien afficher.
4. Pendant une session : `kill -9 $(pgrep -x nox)`. Vérifier que `pgrep -f "lid-guard"` ne
   trouve plus rien après une dizaine de secondes, et que la ligne `SleepDisabled` a disparu.
5. Pendant une session, capot fermé, débrancher avec un seuil supérieur à la charge : le Mac doit
   se mettre en veille sous ~10 s ; Nox doit annoncer « Batterie sous le seuil ».
6. `sudo pmset -a disablesleep 1` **avant** une session, puis arrêter la session : la valeur
   doit rester à `1`. Remettre ensuite `0`.
7. Pendant une session, redémarrer le Mac : au retour, aucune ligne `SleepDisabled`.
8. Retirer l'autorisation depuis Nox : la règle et le LaunchAgent doivent disparaître.
9. Une fois, installer une version qui contient l'ancien helper (`/Library/LaunchDaemons/io.github.martingaltier.nox.helper.plist`) et vérifier qu'il est retiré avec la nouvelle autorisation.
