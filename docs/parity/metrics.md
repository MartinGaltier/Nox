# Parité — Agent Mesures (F1–F10, E2)

Machine : MacBook Air M2 `Mac14,2`, macOS 27.0 (26A428). Vérifications du 2026-10-24,
détail et valeurs comparées dans [`docs/METRICS_SOURCES.md`](../METRICS_SOURCES.md).
Branche `agent/metrics`.

| ID | Mesure | Source Nox | Validation exécutée | Statut |
|---|---|---|---|---|
| F1 | CPU global + par cœur (P/E) | `host_processor_info` Δ ticks ; type par `cluster-type` du registre | vs `top` (33,4 % / 31,7 %) ; 8× `yes` → 100 % ; cpu0–3 E, cpu4–7 P | testée |
| F2 | GPU | `PerformanceStatistics."Device Utilization %"` via `IORegistryEntryCreateCFProperty` | Charge Metal → 99 % (7 W GPU). `ioreg` affiche 0 en permanence sur macOS 27 : non comparable | testée |
| F3 | Températures CPU/GPU/batterie/SSD/Wi-Fi | SMC `flt` ; CPU = max cœurs M2, GPU = max `Tg*` | Plausibles ; CPU 78 → 107 °C sous 8× `yes`, GPU 70 → 86 °C sous Metal (pas de référence CLI sans root) | testée |
| F4 | RAM, pression, swap | `host_statistics64` rev3, `kern.memorystatus_vm_pressure_level`, `vm.swapusage` | Identique à `vm_stat` / `sysctl` à 0,01 Gio près | testée |
| F5 | Batterie | IOPS + `AppleSmartBattery` + SMC `TB*T`/`PDTR` | Charge, secteur, cycles, autonomie identiques à `pmset`/`ioreg` ; santé **estimée** 87,6 % vs 87 % ; **état indisponible** (clés réservées à un entitlement privé) | testée (santé estimée, état indisponible) |
| F6 | Puissance | SMC `PSTR` (système), IOReport `GPU Energy` ; CPU/ANE | Système cohérent avec batterie/adaptateur et sous charge ; GPU 0,05 → 7 W sous Metal ; **CPU/ANE indisponibles** (compteurs IOReport figés sur macOS 27) | testée (CPU/ANE indisponibles) |
| F7 | Débit réseau | `SCDynamicStore` PrimaryInterface + ifmib 64 bits | Totaux identiques à `netstat -ib` (après correction de la troncature 32 bits de `NET_RT_IFLIST2`) | testée |
| F8 | Espace disque | `getfsstat` + filtre local/navigable + `/` | Identique à `df -k /` (au bloc près, écritures concurrentes) ; pas de volume externe testé | testée (`/` seulement) |
| F9 | Historique borné | `MetricsHistory` 7 séries, trous = `None`, `history_len` appliqué à chaud | Tests auto + test matériel `module_lifecycle…` | testée |
| F10 | Ventilateurs | SMC `FNum`/`F<n>Ac` | Aucune clé `F*` sur ce M2 → `Unavailable(NotSupported)` | testée (Indisponible attendu) |
| E2 | Alertes batterie interne | `low_battery_alert` → `AlertKind::LowBattery` une fois par franchissement (réarmement à +3 points ou secteur) ; `AlertKind::PowerSource` à chaque changement de source | Tests unitaires (franchissement, hystérésis, désactivé à 0, secteur) ; pas encore observé en réel avec le module en marche | implémentée |

Coût mesuré : 2,5–4,2 ms CPU par échantillon selon la charge de la machine, 0,12–0,21 %
d'un cœur à 2 s (release, 60 s, 3 mesures) ; ≈ 0,025–0,04 % à l'intervalle d'arrière-plan de 10 s.
