# Journal des sessions Claude

Une ligne par session de travail, **la plus récente en haut** (LIFO). Sert à la
rétrospective : ce que chaque session visait, ce qu'elle a coûté, si elle est
allée au bout.

Rempli par le skill `itsanas` au §6, dans la même PR que le travail. Une session
interrompue avant sa PR est ajoutée par la session suivante (§1), à partir de
l'arbre sale ou de la PR orpheline qu'elle trouve, avec `Fin` = `interrompue`.

## Repérer une session coupée par la limite

Une session coupée n'écrit rien. Deux traces la trahissent, et le skill les
cherche au §1 de la session suivante :

- **un arbre sale ou une PR orpheline** — le travail est là, la ligne manque ;
- **une session de l'app sans ligne ici** — `list_sessions` montre une session
  itsanas dont le `createdAt` ne figure dans aucune `Début` : elle est morte
  avant d'avoir produit quoi que ce soit. Ligne `interrompue`, tokens `n/d`,
  même si rien n'est à reprendre — son coût compte dans l'étape.

## Lire le journal

`python3 scripts/session-cost.py` liste les sessions interrompues et somme les
points 5 h par étape (borne basse marquée `+` quand un coût est `n/d`).

Colonnes :

- **Début** — heure locale de création de la session (`get_session.createdAt`).
- **Durée** — de ce début à l'écriture de la ligne.
- **Tokens** — contexte final de la session (`get_usage.context.tokensUsed`)
  et points de la fenêtre 5 h consommés (`plan` % fin − % début). Approximatif :
  le contexte n'est pas la facturation cumulée ; `n/d` si l'outil manque.
- **Étape** — l'identifiant §8 de `HANDOVER.md` visé (`8.0m`…), ou `hors-§8`.
  C'est la clé de regroupement : le coût d'une feature est la somme de ses
  lignes, sessions interrompues comprises.
- **Fin** — `au bout` (PR fusionnée ou livrable remis) ou `interrompue`.
- **Statut** — ✅ réussie · 🟨 partielle (livrée avec des manques nommés) · ❌ échec.
- **Focus** — une phrase courte.

| Début | Durée | Tokens | Étape | Fin | Statut | Focus | PR |
|---|---|---|---|---|---|---|---|
| 2026-09-29 23:52 | ~15 min | ~120 k ctx · 5 h +13 pts (18→31 %), hebdo +2 pts (96→98 %) | 8.0p (2e tiers) | au bout | 🟨 | Tranche (1)+(4) choisie par Nicolas (hebdo à 96 %) : `itsanas instances` (compte, home, dossier joignable par marqueur, démon par verrou) ; `passphrase` refusé par les 4 scripts ; `.ps1` non testés, pas de Rodin | à venir |
| 2026-09-29 21:42 | ~1 h 30 (dont l'attente d'une réponse) | ~105 k ctx · fenêtre 5 h remise en cours, hebdo +3 pts (92→95 %) | 8.0p (1re moitié) | au bout | 🟨 | Coupée en deux sur décision de Nicolas (quota hebdo à 92 %) : `itsanas --instance NAME` / `ITSANAS_INSTANCE` → `~/.itsanas-NAME` comme provision.sh, nom validé (dont `passphrase` réservé) ; `instances`, migration et refus sans nom restent | #189 |
| 2026-09-29 19:00 | ~15 min | ~70 k ctx · n/d | 8.0o (2b.4) | au bout | ✅ | #187 (2b.3 c) trouvée fusionnée, sa ligne manquait ; point 4 jugé sans objet (la lecture conditionnelle n'apprendrait jamais une machine inscrite plus tard), 0o clos, pointeur sur 0p | #188 |
| 2026-09-29 18:25 | ~20 min | ~160 k ctx · +5 pts/5 h | 8.0o (2b.3 a-b) | au bout | 🟨 | Le carnet garde la claim de chaque adresse (fichier v2, v1 relu), la revérifie au chargement, `relayable()` ; trou Rodin corrigé (signature la plus récente gardée) ; appareil retiré relayable jusqu'à la lecture suivante, nommé | #186 |
| 2026-09-29 17:45 | ~20 min | ~105 k ctx · +5 pts/5 h (fenêtre remise à zéro) | 8.0o (2b.3, 1re moitié) | au bout | 🟨 | Travail interrompu trouvé dans l'arbre (liste du coordinateur avec la claim de chaque machine) revérifié, 5 sabotages rouges ; prose ARCHITECTURE corrigée (repli par raccrochage, trou Rodin) ; claims pas encore gardées dans le carnet | #185 |
| 2026-09-29 14:15 | ~25 min | ~137 k ctx · +8 pts/5 h | 8.0o (2b.2) | au bout | 🟨 | Carnet d'adresses sur disque (présences signées, mémoire de la signature, écriture atomique), revérifié au chargement ; succès horodatés monotones (trou Rodin : Pi en 1970) ; chargement/sauvegarde du démon sans test propre | #184 |
| 2026-09-29 04:45 | ~25 min | ~260 k ctx · +50 pts/5 h | 8.0o (2b.1) | au bout | 🟨 | #182 revérifiée (5 sabotages) et fusionnée ; liste du coordinateur signée et vérifiée, repli non signé fermé après une première signature (trou trouvé par Rodin) ; mémoire de la signature pas encore sur disque (2b.2) | #183 |
| 2026-09-29 00:20 | ~25 min | ~253 k ctx · +32 pts/5 h | 8.0o (2a) | interrompue | 🟨 | #181 revérifié (10 sabotages) et fusionné ; coordinateur contacté seulement si dû (24/jour au lieu de 288) ; gossip signé (2b) non fait, deux correctifs Rodin sans test propre ; arrêtée PR verte et ouverte, fusionnée par la session suivante (ligne écrite « au bout » avant la fusion) | #182 |
| 2026-09-28 23:39 | ~30 min | ~277 k ctx · +30 pts/5 h | 8.0n | au bout | 🟨 | Écriture refusée au-delà de ce que les pledges du compte gagnent (règle corrigée après Rodin) ; moitié disque de 1b non faite | #181 |
| 2026-09-28 22:52 | ~40 min | ~165 k ctx · +10 pts/5 h | 8.0m | au bout | 🟨 | `itsanas leave` : notice pair `Leaving` (v5) + départ signé au coordinateur ; arrêt du service non branché | #180 |
| 2026-09-28 21:15 | ~15 min | ~95 k ctx · +2 pts/5 h | hors-§8 | au bout | ✅ | Création de ce journal et branchement du skill pour le remplir | #177 |
