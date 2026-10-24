# Maintien éveillé capot fermé : fonctionnement, récupération, limites

Ce document couvre le mode **capot fermé** (`KeepAwakeMode::ClosedLid`) et l'assistant
privilégié `nox-helper`. Les deux autres modes (`System`, `SystemAndDisplay`) n'utilisent
que des assertions IOPM détenues par le processus Nox. Le noyau les libère dès que le
processus disparaît, il n'y a donc rien à récupérer pour eux.

> **Statut :** implémenté et testé automatiquement (logique, protocole, serveur réel non
> privilégié avec un `pmset` factice, client tué par `SIGKILL`). **Non testé physiquement**
> capot fermé : l'agent n'a ni installé le helper, ni lancé `sudo`, ni modifié `pmset`.

## 1. Pourquoi un helper root

* Les assertions IOPM (`PreventUserIdleSystemSleep`, `PreventUserIdleDisplaySleep`)
  empêchent seulement la veille **d'inactivité**. À la fermeture du capot d'un MacBook
  sans écran externe, le noyau met le Mac en veille quand même.
* Seul `pmset disablesleep 1` (`SleepDisabled`) empêche cette veille, et il faut être root
  pour le régler.
* Vorssaint installe une règle sudoers `NOPASSWD` pour `pmset disablesleep 0/1`
  (`SudoersSupport.swift`, `ShellSupport.swift`, `KeepAwakeManager.swift`). La restauration
  dépend alors de l'app : marqueur dans UserDefaults, rejoué au lancement suivant.
  **Nox ne modifie jamais sudoers et n'enregistre aucun mot de passe.** Le helper est un
  daemon launchd minimal qui restaure **de lui-même**.

## 2. Compatibilité vérifiée sur ce Mac (macOS 27.0, 26A428, MacBook Air M2)

Commandes lancées par l'agent, toutes sans privilèges :

```
$ ls -l /usr/bin/pmset
-rwxr-xr-x  1 root  wheel  565168  3 sept. 12:34 /usr/bin/pmset
$ strings /usr/bin/pmset | grep -n "disablesleep\|SleepDisabled"
177:SleepDisabled
178: SleepDisabled
575:disablesleep
$ pmset -g          # aucune ligne SleepDisabled : elle n'apparaît que quand la valeur vaut 1
$ ioreg -r -n IOPMrootDomain -d1 -l | grep SleepDisabled
      "SleepDisabled" = No
```

* `pmset` connaît toujours l'argument `disablesleep`.
* `pmset -g` n'affiche ` SleepDisabled 1` que lorsque la valeur est active. Vorssaint lit
  cette sortie.
* Nox lit la valeur **sans processus externe** : propriété `SleepDisabled` de l'entrée
  `IOPMrootDomain` de l'IORegistry (`nox_helper::sys::sleep_disabled`). Sur ce Mac elle
  vaut `false`, ce qui concorde avec `pmset -g`. powerd la publie via
  `IOPMrootDomain::setProperties` (`kIOPMSleepDisabledKey`, source xnu-12377.121.6). Je
  n'ai pas pu vérifier, faute de root, que cette propriété passe à `Yes` après
  `pmset disablesleep 1`. C'est pourquoi le helper confirme chaque écriture en relisant
  pendant 3 s, et renvoie une erreur si la valeur ne change pas.
* La valeur est **persistante** : powerd la stocke dans `SystemPowerSettings`
  (`/Library/Preferences/com.apple.PowerManagement*.plist`). Elle survit au redémarrage,
  d'où la récupération au démarrage du helper (§ 4).

### Ce que `disablesleep` désactive, et ce qu'il ne désactive pas

Lu dans le source xnu (`IOPMrootDomain::checkSystemSleepAllowed`, xnu-12377.121.6) : le
test `userDisabledAllSleep` (posé par `SleepDisabled`) arrive **avant** les exceptions
« batterie faible » et « urgence thermique ». Donc, tant que `SleepDisabled` vaut 1 :

* les veilles d'inactivité, à la fermeture du capot et demandées (menu Pomme) sont refusées ;
* **la veille d'urgence logicielle sur batterie faible et la veille d'urgence thermique
  sont aussi refusées.**

Restent actifs, car hors de cette fonction : la limitation thermique du CPU/GPU (pression
thermique, gérée par le matériel et le noyau) et l'extinction matérielle de protection
(température critique, coupure batterie). Nox ne touche **à aucun réglage thermique**.

Comme `disablesleep` neutralise ces deux protections logicielles, **le helper les remplace
lui-même**, même si l'app est figée :

| Garde (dans le helper) | Déclencheur | Action |
|---|---|---|
| Batterie | sur batterie **et** charge ≤ seuil choisi par l'app (`stop_below_percent`) | restaure `SleepDisabled=0`, révoque le bail (`battery`) |
| Thermique | `com.apple.system.thermalpressurelevel` ≥ 2 (*heavy*) | restaure, révoque (`thermal`) |
| Bail | aucun `heartbeat` depuis `lease` s (60 s), ou connexion fermée et non reprise sous 60 s | restaure (`lease-expired` / `client-gone`) |

Le helper évalue ces gardes toutes les 10 s pendant un bail (IOKit + libnotify, sans
processus externe). Après une restauration, si le capot est **déjà fermé** et que le
noyau mettrait en veille à la fermeture (`AppleClamshellState` et
`AppleClamshellCausesSleep` à `Yes`), il demande la veille (`IOPMSleepSystem`) : macOS
n'évalue le capot qu'au moment où il bouge. Sans cela, un Mac fermé dans un sac pendant
la session y resterait éveillé. Vorssaint fait la même chose, problème n° 1729.

## 3. Architecture

```
Nox.app (utilisateur)                         nox-helper (root, launchd)
  nox-power Engine ── socket Unix ──────────▶ peer check → HelperCore → /usr/bin/pmset -a disablesleep 1|0
   assertion système      /var/run/io.github.martingaltier.nox.helper.sock (root:staff 0660)
   bail : acquire/heartbeat 15 s/release      journal : /Library/Application Support/Nox/helper-state.json (0600)
```

* **Fichiers installés :** `/Library/PrivilegedHelperTools/io.github.martingaltier.nox.helper`
  (root:wheel 0755) et `/Library/LaunchDaemons/io.github.martingaltier.nox.helper.plist`
  (`RunAtLoad`, `KeepAlive`, journal d'erreurs `/Library/Logs/Nox/nox-helper.log`, sans
  donnée personnelle).
* **Protocole** (une ligne ASCII ≤ 128 octets par requête, liste blanche stricte) :
  `version`, `status`, `acquire <lease 5..600> <batterie 0..100>`, `heartbeat`,
  `release`. Toute autre ligne renvoie `err bad-request`. Le helper n'exécute qu'un seul
  programme, `/usr/bin/pmset`, par chemin absolu, avec des arguments fixes et un
  environnement vide.
* **Vérification du pair :** uid effectif (`getpeereid`) égal à celui de la session
  console (propriétaire de `/dev/console`) ou à root. Ensuite, le chemin de l'exécutable
  client (`LOCAL_PEERPID` puis `proc_pidpath`, chemin canonique) doit être celui de l'app
  pour laquelle le helper a été installé (`--allowed-client`). Si ce n'est pas le cas, la
  réponse est `err forbidden` et l'UI affiche `HelperStatus::ClientRejected` (« réinstallez »).
* **Règles de restauration** (`nox_helper::core`, tests unitaires) :
  1. Le journal (valeur initiale `0`) est écrit puis synchronisé sur disque (`fsync` +
     `rename`) **avant** l'appel à `pmset`.
  2. Si la valeur initiale vaut déjà `1`, Nox **n'écrit rien**, ni à l'activation ni à
     l'arrêt.
  3. Avant de restaurer, le helper **relit** la valeur. Si elle vaut déjà `0` (changée par
     l'utilisateur), il n'écrit rien et efface le journal. Il fait de même pendant un bail
     (révocation `external`).
  4. Si la relecture est impossible, il restaure `0` par sécurité. Un journal corrompu est
     traité comme « Nox a posé 1 ».
  5. Si `pmset` échoue, le journal est conservé et le helper réessaie toutes les 30 s.

## 4. Comportement après incident

| Incident | Ce qui se passe |
|---|---|
| Arrêt normal / Quitter | `Engine::stop` → `release` → le helper restaure. |
| Crash de Nox, `kill -9`, arrêt forcé | Le noyau ferme la socket ; après 60 s sans reconnexion, le helper restaure. Au relancement de Nox, `startup_recovery()` voit le marqueur `~/Library/Application Support/Nox/power/closed-lid-lease.json` et envoie `release` tout de suite, puis affiche une notice. |
| Nox figé (socket ouverte, plus de heartbeat) | Après 60 s, le helper restaure (`lease-expired`). |
| Mac débranché ou batterie basse, app figée | Garde batterie du helper (≤ 10 s). |
| Crash du helper | launchd le relance (`KeepAlive`). Au démarrage, journal présent → restauration. L'app reconnecte au heartbeat suivant et reprend un bail. |
| Redémarrage / coupure de courant | Au démarrage, launchd lance le helper (`RunAtLoad`) : journal présent → restauration (`SleepDisabled` est persistant). |
| `launchctl bootout` / désinstallation | `SIGTERM` → le helper restaure avant de quitter. Le script de désinstallation lance ensuite `--recover` si un journal subsiste. |

## 5. Récupération manuelle

```sh
# 1. État actuel (aucune ligne = veille normale)
pmset -g | grep SleepDisabled
ioreg -r -n IOPMrootDomain -d1 -l | grep SleepDisabled

# 2. Rétablir la veille capot fermé
sudo pmset -a disablesleep 0

# 3. Arrêter et retirer le helper (il restaure s'il avait modifié la valeur)
sudo launchctl bootout system/io.github.martingaltier.nox.helper
sudo rm -f /Library/LaunchDaemons/io.github.martingaltier.nox.helper.plist \
           /Library/PrivilegedHelperTools/io.github.martingaltier.nox.helper \
           /var/run/io.github.martingaltier.nox.helper.sock \
           "/Library/Application Support/Nox/helper-state.json"
rm -f "$HOME/Library/Application Support/Nox/power/closed-lid-lease.json"

# ou, depuis le dépôt :
sudo scripts/uninstall-helper.sh
```

Journal du helper : `/Library/Logs/Nox/nox-helper.log`.

## 6. Installation

* Uniquement sur action explicite (`PowerCommand::InstallHelper`, bouton « Installer
  l'assistant »). Nox lance **une fois** `osascript -e 'do shell script "…" with
  administrator privileges'`. Le mot de passe est saisi dans la boîte de dialogue système :
  Nox ne le voit pas et ne le stocke pas. Le script exécuté est
  `scripts/install-helper.sh`, **embarqué dans le binaire signé** (`include_str!`). Il n'y
  a pas de fichier temporaire modifiable entre l'écriture et l'exécution. Quitter Nox
  pendant la boîte de dialogue tue le processus `osascript`, ce qui abandonne la demande
  (non vérifié avec une vraie boîte de dialogue).
* Le script vérifie la signature du helper embarqué (`codesign --verify --strict`), le
  copie depuis `Nox.app/Contents/Library/LaunchServices/nox-helper`, fait générer le plist
  par le helper lui-même (échappement XML testé, `plutil -lint`), puis lance
  `launchctl enable` et `bootstrap system` et attend la socket.
* À la main : `sudo scripts/install-helper.sh /Applications/Nox.app`.
* Désinstallation : bouton « Désinstaller » (`UninstallHelper`), ou
  `sudo scripts/uninstall-helper.sh`.
* `HelperStatus` est détecté sans privilèges : présence des deux fichiers, puis
  `version` sur la socket (`Installed` / `Outdated` / `Unreachable` / `ClientRejected`).

### Pourquoi pas SMAppService.daemon (analyse, non testé)

* Le daemon doit rester **dans le bundle** (`BundleProgram`). Si l'app est déplacée ou
  supprimée, la restauration au démarrage ne s'exécute plus, alors que c'est elle qui
  garantit G9.
* L'app est signée ad hoc, sans Team ID. Background Task Management associe
  l'autorisation à l'identité de code, qui change à chaque recompilation ad hoc : il
  faudrait réapprouver dans Réglages > Ouverture à la connexion. Il n'est pas non plus
  possible d'exiger une signature précise du client.
* `osascript … with administrator privileges` fonctionne sans identité de développeur, et
  le binaire copié dans `/Library/PrivilegedHelperTools` ne dépend plus de
  l'emplacement de l'app.

## 7. Menaces et limites

* **Usurpation par un processus du même utilisateur.** La vérification du chemin client
  passe par `pid → proc_pidpath`. Elle n'est pas atomique avec la connexion (réutilisation
  de pid théorique), et sans Team ID on ne peut pas exiger de signature. Un processus
  malveillant du même utilisateur qui remplacerait le binaire de Nox.app (dossier
  modifiable par un admin) pourrait obtenir un bail. Le pire qu'il puisse faire est
  l'opération autorisée elle-même : empêcher la veille capot fermé tant qu'il envoie des
  heartbeats, toujours sous les gardes batterie et thermique du helper. Il ne peut pas
  exécuter de commande arbitraire.
* **Changement de session (Fast User Switching).** Le bail est conservé. Le nouvel
  utilisateur console ne peut pas le prendre (`busy`) tant que le propriétaire est
  connecté. Si le propriétaire cesse ses heartbeats, le helper restaure à l'expiration
  du bail.
* **Une valeur `1` posée par l'utilisateur pendant que Nox tient un bail** est
  indiscernable de celle de Nox : elle sera ramenée à `0` à la fin du bail. En revanche,
  une valeur `0` posée pendant le bail est respectée, puisque le helper révoque le bail.
* **Garde thermique :** si la pression thermique atteint *heavy* (calcul lourd, capot
  ouvert), la veille capot fermé est rétablie et la session s'arrête (`StopReason::Thermal`).
  C'est volontaire.
* **Sommeil forcé :** pendant un bail, `SleepDisabled` bloque aussi la veille demandée
  depuis le menu Pomme. C'est le comportement de macOS.
* **Délai de grâce de 60 s :** après un crash, la veille capot fermé reste désactivée
  jusqu'à 60 s, sauf si Nox est relancé avant.

## 8. À tester physiquement (par l'utilisateur)

1. Construire `dist/Nox.app` et l'installer dans `/Applications`. Cliquer « Installer
   l'assistant », saisir le mot de passe, puis vérifier :
   `sudo launchctl print system/io.github.martingaltier.nox.helper | grep state` et
   `ls -l /var/run/io.github.martingaltier.nox.helper.sock`.
2. Mode capot fermé, 15 min, sur secteur sans écran externe. Contrôler :
   `pmset -g | grep SleepDisabled` doit afficher `1`, et
   `ioreg -r -n IOPMrootDomain -d1 -l | grep SleepDisabled` doit afficher `Yes`. Fermer le
   capot 2 min avec par exemple `ping` vers le Mac depuis un autre appareil : il doit
   rester joignable.
3. Arrêt manuel, puis `pmset -g | grep SleepDisabled` : aucune ligne.
4. Pendant une session : `kill -9 $(pgrep -x nox)`, attendre 70 s, puis vérifier que la
   ligne SleepDisabled a disparu.
5. Pendant une session, capot fermé, débrancher le Mac avec un seuil supérieur à la
   charge : il doit se mettre en veille sous ≤ 10 s environ.
6. `sudo pmset -a disablesleep 1` **avant** de lancer une session, puis arrêter la
   session : la valeur doit rester à `1` (Nox ne l'a pas touchée). Remettre ensuite `0`.
7. Pendant une session, redémarrer le Mac : au retour, aucune ligne SleepDisabled.
8. Désinstaller depuis l'app, puis vérifier que les deux fichiers ont disparu.
