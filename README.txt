Patch à appliquer sur graine-no-auth-update.zip (ou le dépôt actuel).

- web/src/App.tsx : coins arrondis, hauteur exacte des dix premières lignes, Leaderboard limité aux 100 premiers torrents.
- web/src/components/ui/table.tsx : mesure dynamique de dix lignes avec ResizeObserver.

Aucune modification Rust/SQLite : le plafond de 100 ne s’applique qu’à l’affichage ; les historiques et les torrents supprimés sont conservés.
