# Référence Vorssaint épinglée

| Élément | Valeur |
|---|---|
| Dépôt | https://github.com/vorssaint/vorssaint-utils |
| Commit | `e916961092686d32d455d12494ab10f65a43b73a` |
| Date | 2026-10-22 18:14:09 -0300 |
| Message | docs: summarize reviewed pull requests and contributors |
| Licence | GPL-3.0-or-later (+ `TRADEMARKS.md` : pas de nom/icône/bundle/branding Vorssaint) |
| Langage | Swift 5.9, SwiftPM, macOS 14+, Apple Silicon |

Vérification reproductible :

```sh
git clone https://github.com/vorssaint/vorssaint-utils && cd vorssaint-utils
git checkout e916961092686d32d455d12494ab10f65a43b73a
```

Périmètre étudié : `Sources/Vorssaint/Services/Notch`, `UI/Notch`, `Services/AgentUsage`,
`Services/Metrics`, `Services/SystemMonitor`, `Services/KeepAwakeManager.swift`,
`Services/SudoersSupport.swift`, `Sources/NowPlayingAdapter`, `Core/Defaults.swift`
(clés `notch*`), `Core/Notch*Strings.swift` (≈ 42 000 lignes pour l'îlot seul).

Toute évolution ultérieure de Vorssaint est hors périmètre tant que ce commit n'est pas
explicitement mis à jour dans ce fichier.
