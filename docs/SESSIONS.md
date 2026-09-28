# Journal des sessions Claude

Une ligne par session de travail, **la plus récente en haut** (LIFO). Sert à la
rétrospective : ce que chaque session visait, ce qu'elle a coûté, si elle est
allée au bout.

Rempli par le skill `itsanas` au §6, dans la même PR que le travail. Une session
interrompue avant sa PR est ajoutée par la session suivante (§1), à partir de
l'arbre sale ou de la PR orpheline qu'elle trouve, avec `Fin` = `interrompue`.

Colonnes :

- **Début** — heure locale de création de la session (`get_session.createdAt`).
- **Durée** — de ce début à l'écriture de la ligne.
- **Tokens** — contexte final de la session (`get_usage.context.tokensUsed`)
  et points de la fenêtre 5 h consommés (`plan` % fin − % début). Approximatif :
  le contexte n'est pas la facturation cumulée ; `n/d` si l'outil manque.
- **Fin** — `au bout` (PR fusionnée ou livrable remis) ou `interrompue`.
- **Statut** — ✅ réussie · 🟨 partielle (livrée avec des manques nommés) · ❌ échec.
- **Focus** — une phrase courte.

| Début | Durée | Tokens | Fin | Statut | Focus | PR |
|---|---|---|---|---|---|---|
| 2026-09-28 21:15 | ~15 min | ~95 k ctx · +2 pts/5 h | au bout | ✅ | Création de ce journal et branchement du skill pour le remplir | #177 |
