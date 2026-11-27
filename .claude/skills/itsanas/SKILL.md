---
name: itsanas
description: "Reprendre ou poursuivre ITSaNAS (D:\\GitHub\\itsanas) : lit le pointeur de docs/HANDOVER.md, termine le travail en cours, enchaîne les points de §8 en plusieurs fonctions par PR, décide seul quand faire une passe red team ou une consultation Rodin, et tient le handover court (l'historique part dans HANDOVER-ARCHIVE.md). À invoquer sans argument."
---

# ITSaNAS — reprise de chantier

Stockage P2P chiffré, Rust, dépôt `D:\GitHub\itsanas` (remote `SigSegGit/itsanas`,
public, AGPL-3.0). Nicolas décide ; tu es architecte et développeur principal.
`AGENTS.md` fait foi pour les règles du dépôt public, la fusion et la CI ; ce
skill dit comment mener une session.

**Nicolas ne te dit jamais où on en est.** Sans argument : « reprends ». Avec un
argument : l'objectif de la session, mais tu lis le pointeur d'abord.

## 1. Savoir où on en est

- **L'intention** : le bloc `ITSANAS-STATE` de `docs/HANDOVER.md` §0 (`NEXT`,
  `TITLE`, `WRITTEN-AT`, `BASE`) et la section « State now » juste dessous.
- **Les faits** : `git status`, `git log --oneline -20 origin/main`,
  `gh pr list`, `gh run list`. Les faits gagnent : un `NEXT` déjà fusionné, tu
  avances au point suivant de §8 et tu corriges le pointeur en fin de session.
- **Un arbre sale ou une PR ouverte passent d'abord** : on les mène au vert
  (§3 et §4 s'appliquent, sabotage compris) avant toute nouvelle étape.

Annonce en trois lignes (dernier commit sur `main`, état de l'arbre, ce que tu
fais), puis enchaîne. Tu n'interromps Nicolas que si le handover se contredit
sur le fond : question fermée, deux options, leurs conséquences.

## 2. Travailler : plusieurs fonctions par PR

- Une session enchaîne les points de §8 jusqu'au bout de la liste ou du budget ;
  une PR porte plusieurs fonctions qui vont ensemble. La règle « une fonction par
  session » est abolie (Nicolas, 2026-11-15 et 2026-11-27 : elle gaspillait et
  ralentissait).
- Chaque changement de comportement part d'un test qui échoue sans lui ; un
  test red-team est sabotage-vérifié (`scripts/sabotage.py`).
- « Right the first time » d'`AGENTS.md`, puis `scripts/preflight.sh` une fois.
- **Pas de fichier source au-delà de 1 000 lignes** (`scripts/check-file-size.py`).
  Un ajout va dans le module où il appartient ; si ce module n'existe pas, tu le
  crées. Un fichier de la liste `CEILING` que tu touches, tu le découpes dans la
  même PR si le découpage est mécanique, sinon tu ne le fais pas grossir.
- PR ouverte → prête + auto-merge squash, et tu passes à la suite sans regarder
  la CI (`AGENTS.md`, « Merge without asking »). Un rouge se lit une fois et se
  corrige en un seul push.

## 3. Quand faire une passe red team ou une consultation Rodin

C'est toi qui décides, sans demander, selon ces déclencheurs ; tu inscris chaque
passe dans §0 (« Last passes ») avec sa date et sa PR.

| Passe | Déclencheur | Forme |
|---|---|---|
| **Red team du diff** | avant le push d'une PR qui touche le protocole pair ou coordinateur, la crypto ou les clés, l'autorisation, la suppression / la synchro / la corbeille, une frontière FFI ou `unsafe`, l'installation ou la mise à jour | agent `redteam` (`.claude/agents/`) sur le diff ; chaque constat corrigé dans la PR ou écrit en §9 avec sa raison |
| **Rodin sur le plan** | avant de coder un point de §8 qui n'a pas de spécification écrite, ou qui change une décision de §6 | skill `rodin:rodin` sur le plan, pas sur le code |
| **Rodin sur le projet** | toutes les 10 PR fusionnées depuis la dernière consultation, ou quand une étape a coûté deux tours de CI rouges ou plus | consultation complète ; ses constats deviennent des points de §8 |
| **Passe complète** | toutes les 25 PR fusionnées, ou avant une release annoncée | workflow : lentilles par sous-système, chaque constat contre-vérifié par un sceptique, puis corrections test d'abord |

Rien de tout ça pour une PR de docs seules ou d'interface sans logique. Le
reste du temps, pas de sous-agent : une session lit et écrit elle-même.

## 4. Tenir le handover court

`docs/HANDOVER.md` est un handover, pas un journal. À chaque PR :

1. **§0** : le bloc `ITSANAS-STATE` à jour, puis « State now » en **40 lignes au
   plus** : ce qui vient d'être fusionné (une ligne, PR), ce qui est en vol, ce
   qui suit, « Last passes », et seulement les pièges qui mordent encore.
2. **Le récit de session** (ce qui a été fait, trouvé, raté) part en tête de
   `docs/HANDOVER-ARCHIVE.md`, daté, pas dans §0.
3. **§8 ne garde que le travail ouvert** : chaque point en ~15 lignes (but, ce qui
   est construit avec le nom du code ou du test, ce qui reste, ce qu'il attend).
   Un point fini part **verbatim** dans l'archive ; il ne reste qu'une ligne ✅
   s'il est encore cité ailleurs (`§8 15` dans un commentaire, par exemple).
4. **§9 / §10** : un trou refermé ou une question tranchée part dans l'archive.
5. `scripts/check-handover.py` doit passer ; il borne la taille de §0 et du
   fichier.

Critère de sortie : relis §0 et §8 comme si tu n'avais rien vu de la session. Si
tu ne peux pas démarrer, ils ne sont pas finis.

## 5. Les règles non négociables

- Les docs changent dans le même commit que le code ; un document qui contredit
  le code est un bug. Un présent de l'indicatif veut dire « un test nommé le
  prouve aujourd'hui ».
- Un test ajouté a sa ligne de catalogue dans `docs/TESTING.md`, ses compteurs
  (`check-counts.py` les donne), et sa surface s'il est red team
  (`scripts/redteam_surfaces.py`).
- Dépôt public : les six règles d'`AGENTS.md`. Aucun nom de personne privée
  (le partenaire de test s'appelle « the partner »), aucune machine, aucun port,
  aucun jeton.
- Suite de tests : `cargo nextest run --profile ci --workspace --all-features`,
  avec `CARGO_TARGET_DIR=D:/GitHub/itsanas/target` depuis un worktree.
- Les scripts d'édition s'écrivent en Python dans le scratchpad : les heredocs de
  Git Bash mangent les antislashs.

## 6. Ton

Du couteau, pas de la ouate. Décide seul sur la routine et signale la décision.
Signale surtout ce que **tu** as raté.
