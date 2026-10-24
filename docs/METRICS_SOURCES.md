# Sources des mesures (`nox-metrics`)

Machine de vérification : MacBook Air M2 (`Mac14,2`, 4 P + 4 E, 16 Go, **sans ventilateur**),
macOS 27.0 (26A428). Vérifications exécutées le **2026-10-24** (≈ 01:20–06:00), machine
chargée par d'autres compilations (CPU 30–50 % hors tests de charge).

Principes :

* Un seul thread (`nox-metrics` worker) ouvre chaque handle une fois (connexion SMC, abonnement
  IOReport, entrées de registre GPU/batterie, session `SCDynamicStore`) et les libère quand le
  thread se termine (`Module::stop` joint le thread). Aucun processus externe, pas de root,
  pas d'entitlement privé.
* Fréquence : `metrics.interval_ms` (2 s) tant que l'UI a envoyé `Command::MetricsVisible(true)`,
  sinon `metrics.background_interval_ms` (10 s). Passage à visible ou changement de réglages :
  `CancelToken::nudge` → échantillon immédiat, nouvel intervalle sans redémarrage.
* Mesures par différence (CPU, réseau, énergie) : le premier échantillon et tout échantillon
  après un trou > 2,5 × l'intervalle le plus long + 1 s (veille, pause) ne servent que de base →
  `Unavailable(Pending)` et `None` dans l'historique.
* `Reading::Estimated` = valeur modélisée ; `Unavailable(raison)` sinon, jamais 0 à la place
  d'une erreur.
* Outils : `cargo run -p nox-metrics --example dump [--json]` (snapshot réel),
  `--example watch [s] [ms]` (une ligne par échantillon), `--example profile` (coût par source),
  `--example smc_keys [PRÉFIXE…]`, `--example ioreport [ms]`.

## Tableau

| Mesure | Source exacte | Unité | Indisponible quand | Vérification croisée | Résultat sur ce M2 (2026-10-24) |
|---|---|---|---|---|---|
| CPU global | `host_processor_info(PROCESSOR_CPU_LOAD_INFO)`, Δ ticks (user+system+nice)/(total), pondéré par cœur | % | erreur Mach ; 1er échantillon / après trou → Pending | `top -l 2 -n 0 -s 1` | Nox 33,4 % vs `top` 31,7 % (fenêtres décalées de ~1 s). Charge 8× `yes` : 99,8–100,0 % |
| CPU par cœur + type | idem par CPU ; type par la propriété `cluster-type` (`E`/`P`) des nœuds `IOPlatformDevice` indexés par `logical-cpu-id` ; repli `hw.perflevel1.logicalcpu` cœurs E en premier | % | idem | `ioreg -rc IOPlatformDevice -k cluster-type` | cpu0–3 = E, cpu4–7 = P (registre), 4 E = `hw.perflevel1.logicalcpu` (test `snapshot_is_plausible`) |
| GPU | `IORegistryEntryCreateCFProperty(IOAccelerator, "PerformanceStatistics")["Device Utilization %"]` (seule cette propriété est copiée) | % | aucun accélérateur / clé absente → NotSupported | `ioreg -r -c AGXAccelerator -k PerformanceStatistics` | **`ioreg` affiche toujours 0** sur macOS 27 (propriétés sérialisées), alors que `IORegistryEntryCreateCFProperty` (Nox, et un programme C témoin) renvoie 4 % au repos. Charge Metal (noyau de calcul, 16 s) : Nox **99 %**, puissance GPU 7,0 W, T° GPU 76 → 86 °C ; `ioreg` toujours 0. La valeur de Nox suit la charge réelle ; `ioreg` n'est pas une référence utilisable ici |
| Température CPU | SMC `AppleSMC` (sélecteur 2, lecture seule), clés `flt ` ; **max** des capteurs cœurs connus de la puce (M2 : Tp01 Tp05 Tp09 Tp0D Tp0X Tp0b Tp0f Tp0j Tp1h Tp1l Tp1p Tp1t — 7 présentes ici) ; repli : max des autres `Tp*`/`Te*` (lus seulement si aucun cœur ne répond) ; plage plausible 10–125 °C | °C | SMC inaccessible → Error ; aucun capteur plausible → NotSupported | aucune commande sans sudo (`powermetrics` exige root) → cohérence sous charge | Repos chargé 78–80 °C ; 8× `yes` : 99,7 → 107 °C en 15 s (seuil de régulation M2 ≈ 105 °C) ; redescend après arrêt |
| Température GPU | max des clés `Tg*` (`flt `, 6 ici : Tg0e Tg0f Tg0m Tg0n Tg0q Tg0r) | °C | idem | cohérence sous charge | 70 → 86 °C sous charge Metal |
| Autres capteurs | `TB0T TB1T TB2T` (batterie, −10–80 °C), `TH0T TH0x` (SSD/NAND), `TW0P` (module Wi-Fi). Les clés de sens incertain (`Ts*`, `TPD*`, `TRD*`, `TVM*`, `TAOL`…) ne sont pas exposées | °C | — | — | Batterie 29–32 °C, SSD 33–35 °C, Wi-Fi 35 °C |
| Mémoire | `host_statistics64(HOST_VM_INFO64)` (struct rev3 de macOS 26+, compte rendu vérifié) ; `hw.memsize`, `hw.pagesize` ; App = internal − purgeable ; Utilisée = App + câblée + compressée (+ tag storage) ; Cache = file-backed + purgeable (formule Moniteur d'activité) | octets | erreur Mach | `vm_stat`, `sysctl hw.memsize` | `vm_stat` : câblée 136 531 p × 16 Kio = 2,08 Gio (Nox 2,08) ; compresseur 393 009 p = 6,00 Gio (Nox 6,00) ; anonymes − purgeables = 3,56 Gio (Nox app 3,55) ; file-backed + purgeables = 3,73 Gio (Nox 3,73) |
| Pression mémoire | `kern.memorystatus_vm_pressure_level` 1/2/4 → Normal/Warning/Critical | — | autre valeur / erreur | `sysctl kern.memorystatus_vm_pressure_level` | 2 → Warning des deux côtés |
| Swap | `vm.swapusage` (`xsw_usage`) | octets | — | `sysctl vm.swapusage` | 719,1 Mio utilisés / 1024 Mio (identique) |
| Batterie : charge, secteur, en charge | IOPowerSources : `IOPSCopyPowerSourcesInfo` + `IOPSGetPowerSourceDescription` (`Current Capacity`/`Max Capacity`, `Is Charging`), `IOPSGetProvidingPowerSourceType` ; repli registre `AppleSmartBattery` | %, bool | pas de batterie (`BatteryInstalled` = No) → SourceMissing("Batterie interne") | `pmset -g batt` | Sur batterie : 47 %, « discharging » (identique) ; sur secteur : 100 %, « charged », `AC Power` (identique) |
| Cycles | registre `AppleSmartBattery` `CycleCount` | — | clé absente → NotSupported | `ioreg -rn AppleSmartBattery`, `system_profiler SPPowerDataType` | 276 (identique) |
| Santé | 1) IOPS `BatteryPacks[].BatteryHealth."Maximum Capacity Percent"` (Measured) ; 2) sinon `BatteryData.NominalChargeCapacity / DesignCapacity` (**Estimated**, plafonné à 100) | % | ni l'un ni l'autre → NotSupported | Informations système « Capacité maximale » | La clé 1) n'est fournie qu'aux clients munis de l'entitlement privé `com.apple.private.iokit.batterydata` (c'est ce qu'utilise `pmset -g ps -xml`) : absente pour Nox. Estimation 3995/4563 = **87,6 %** (≈ 88 %) vs Informations système **87 %**. Choix de NominalChargeCapacity (capacité pleine lissée) plutôt que FullChargeCapacity (3868 → 84,8 %, varie avec température et débit) |
| État | IOPS `BatteryHealthCondition` / `BatteryHealth`, sinon `Battery Service State` (0 → « Normale ») | texte | aucune de ces clés → NotSupported | Informations système « État » | Sur macOS 27 ces clés ne sont pas publiées sans entitlement → **Indisponible** (Informations système : « Normal »). Pas d'état déduit de la santé (ce serait inventé) |
| Autonomie | IOPS `Time to Empty` (sur batterie) / `Time to Full Charge` (en charge), minutes — **Estimated** ; négatif = calcul en cours → Pending ; secteur sans charge → NotSupported | min | — | `pmset -g batt` | 254 min = « 4:14 remaining » (identique) ; secteur chargé : NotSupported (`pmset` : « 0:00 remaining ») |
| Puissance batterie | registre `Voltage` (mV) × `Amperage` (mA signé, − = décharge) | W | clés absentes → NotSupported | `ioreg -rn AppleSmartBattery` | 11,514 V × −0,199 A = −2,29 W (= `PowerTelemetryData.SystemLoad` 2291 mW). **Limite** : le firmware ne met ces champs à jour qu'environ une fois par minute (`UpdateTime`) ; la clé SMC `PPBR` suit `PSTR` sur batterie mais vaut 0,49 W sur secteur batterie pleine (sens non établi) → non utilisée |
| Puissance adaptateur | SMC `PDTR` (DC-in total), plausible 0–500 W, uniquement sur secteur | W | sur batterie → SourceMissing("Adaptateur secteur") | cohérence avec `PSTR` sur secteur batterie pleine | `PDTR` 22,7 W vs `PSTR` 23,0 W ; 21,1 / 21,3 ; 23,5 / 23,0 |
| Puissance système | SMC `PSTR` (System Total Power), plausible 0–1000 W ; repli **Estimated** = −puissance batterie sur batterie | W | — | sur batterie : V×I batterie ; sur secteur : `PDTR` | Sur batterie au repos : `PSTR` 2,06–2,31 W vs `PPBR` 2,01–2,20 W ; sur secteur : ≈ `PDTR` ; charge CPU : 11 → 18 W ; charge GPU : +6–7 W |
| Puissance CPU / ANE | libIOReport (privée, sans root) groupe « Energy Model », abonnement restreint aux canaux `CPU Energy` et `ANE` (mJ) ; W = Δénergie / Δt | W | **compteur figé** : 2 échantillons sans progression → NotSupported ; ANE n'est cru que si `CPU Energy` progresse (l'ANE est légitimement à 0 au repos) | `--example ioreport` | **Indisponible sur ce M2/macOS 27** : tous les canaux en mJ (`CPU Energy`, `ECPU*`, `PCPU*`, `ANE`, `DRAM`…) restent à leur total depuis le démarrage (CPU Energy = 15 693 752 mJ inchangé sur 12 s, et sous charge 100 %) |
| Puissance GPU | même abonnement, canal `GPU Energy` (nJ), qui progresse | W | idem | cohérence sous charge | Repos 0,04–0,08 W ; charge Metal 6,96–6,99 W (`PSTR` +6 W) |
| Réseau | interface principale = `SCDynamicStoreCopyValue("State:/Network/Global/IPv4").PrimaryInterface` ; compteurs 64 bits `sysctl net.link.generic.ifdata.<index>.general` (`ifmibdata.ifmd_data.ifi_ibytes/obytes`) ; débit = Δoctets/Δt ; totaux = compteurs de l'interface depuis le démarrage | o/s, o | hors ligne → SourceMissing("Connexion réseau") ; changement d'interface / remise à zéro → Pending | `netstat -ib -I en0` | Totaux **identiques** (rx 4 955 222 194, tx 314 364 430). Débit Nox 7,6 ko/s vs `netstat` 9,4 ko/s sur une fenêtre décalée. **Piège évité** : `NET_RT_IFLIST2` renvoie sur macOS 27 des compteurs tronqués à 32 bits (659 Mo au lieu de 4,95 Go = 659 Mo + 2³²) |
| Disques | `getfsstat(MNT_NOWAIT)` ; montrés : `/` toujours + volumes `MNT_LOCAL` sans `MNT_DONTBROWSE` (exclut `/System/Volumes/*`, Recovery, devfs, autofs, partages réseau) ; libre = `f_bavail × f_bsize` ; nom `getattrlist(ATTR_VOL_NAME)` | octets | erreur `getfsstat` | `df -k /` | `/` « Macintosh HD » : 1 472 671 744 o libres vs `df` 1 438 104 Kio = 1 472 618 496 o (écart = écriture entre les deux appels) ; total 245 107 195 904 o = 239 362 496 Kio (identique). Le `/` scellé en lecture seule partage le conteneur APFS des données : son espace libre est celui du disque |
| Ventilateurs | SMC `FNum` puis `F<n>Ac` | tr/min | `FNum` absent ou 0 → NotSupported | `--example smc_keys F` | Aucune clé `F*` parmi les 1725 clés SMC → **NotSupported** (Air sans ventilateur) |

## Coût du collecteur

`cargo test -p nox-metrics --release --test hardware -- --ignored --nocapture collector_cost`
(60 s d'échantillonnage à 2 s, temps CPU du thread via `CLOCK_THREAD_CPUTIME_ID`) :

| Version | Échantillons | CPU total | Par échantillon | Charge moyenne |
|---|---|---|---|---|
| initiale (`NET_RT_IFLIST2` + `if_indextoname`, IOReport groupe entier, tous les `Tp*`) | 30 | 125,9 ms | 4,20 ms | 0,21 % d'un cœur |
| + canaux IOReport filtrés, `Tp*` de repli à la demande | 30 | 85,3 ms | 2,84 ms | 0,14 % d'un cœur |
| actuelle (+ compteurs réseau ifmib de la seule interface principale) | 30 | 74,7 ms | 2,49 ms | **0,12 %** d'un cœur |
| actuelle, re-vérification sur target propre (load average 5–6, autres compilations) | 30 | 98,4 ms / 125,3 ms | 3,28 / 4,18 ms | 0,16 % / 0,21 % |

Le coût varie avec la charge de la machine (cache, contention) : **2,5–4,2 ms par échantillon,
0,12–0,21 % d'un cœur à 2 s**, toujours sous le seuil de 1 % du test.

Mise en route (énumération des 1725 clés SMC, une fois) : ≈ 50–60 ms CPU.
En arrière-plan (10 s) : ≈ 0,025–0,04 % d'un cœur (extrapolé : 2,5–4,2 ms / 10 s). Coût par source (`--example profile`, release,
moyenne de 30 appels) : abonnement IOReport 3 canaux ≈ 640 µs, lecture SMC ≈ 19 µs × 21 clés,
registre batterie (10 propriétés) ≈ 50 µs, IOPS ≈ 32 µs, `PerformanceStatistics` ≈ 29 µs,
compteurs réseau ≈ 32 µs, interface principale (configd) ≈ 14 µs, `getfsstat` ≈ 7 µs,
ticks CPU ≈ 7 µs, VM ≈ 1 µs.

## Tests

* Unitaires (fixtures synthétiques, `cargo test -p nox-metrics`) : décodage SMC (`flt`, `sp78`,
  `fpe2`, `ui*`, `ioft`, tailles invalides, NaN), Δ CPU (bouclage 32 bits), types de cœurs,
  formule mémoire (valeurs `vm_stat` réelles), batterie (registre + IOPS du M2, repli santé,
  charge, sans batterie), réseau (base, trou, remise à zéro, hors ligne), disques (filtrage),
  capteurs (classification M2, agrégation, plausibilité), énergie (compteurs figés / vivants),
  alertes (franchissement unique, hystérésis, secteur), historique (trous = `None`).
* Matériel (`cargo test -p nox-metrics -- --ignored`, exécutés sur ce M2 le 2026-10-24) :
  `snapshot_is_plausible` ✔, `module_lifecycle_publishes_and_cleans_up` ✔ (publication,
  intervalle visible appliqué après `MetricsVisible(true)`, `history_len` appliqué sans
  redémarrage, `metrics = None` et plus aucune publication après `stop`),
  `collector_cost_60s_at_2s` ✔ (0,12–0,21 %). Tous relancés le 2026-10-24 ≈ 10:40 sur un
  dossier de build propre (`target/wt-metrics`) après le merge d'`origin/main`.

## Limites connues

* Puissance CPU/ANE indisponible sur macOS 27 (compteurs IOReport figés) ; réévaluée
  automatiquement à chaque démarrage (si les compteurs progressent, la valeur devient Measured).
* Santé batterie = estimation (≈ +0,6 point vs Informations système) ; état batterie indisponible.
* Puissance batterie rafraîchie par le firmware ~1×/min.
* Alertes batterie (seuil, changement de source) vérifiées par tests unitaires ; pas encore
  observées en conditions réelles (branchement/débranchement pendant que le module tournait).
* Pas de volume externe monté pendant la vérification (filtrage testé sur fixtures).
