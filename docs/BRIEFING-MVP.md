# Protocole de test réel : le MVP, puis la question « est-ce que ça passe à l'échelle ? »

Pour Nicolas, sur ses machines. Deux questions différentes, et ce protocole les
garde séparées exprès :

1. **Le MVP est-il atteint ?** Une personne, ses machines, un réseau domestique.
   Réponse par la règle de verdict écrite *avant* ([MVP.md](MVP.md) §4).
2. **Est-ce que ça peut devenir un réseau à la Storj ?** Beaucoup d'inconnus,
   Internet, des téraoctets. Trois machines ne peuvent pas y répondre ; elles
   peuvent mesurer les chiffres qui disent si ça vaut la peine d'essayer. La
   partie 6 dit précisément ce qui reste inconnu.

Chaque test du kit renvoie `PASS` ou `FAIL` avec les chiffres via
`scripts/acceptance.sh` et s'ajoute à `~/.itsanas-receipts/acceptance.txt`. **Tu
colles ces fichiers, pas une impression.** Un test qui demande un contournement,
un indice ou un second essai est un échec (MVP.md §3).

Compte ~2 jours calendaires : une matinée active (parties 2 à 4), puis des
périodes où les machines tournent seules (H 24 h, I jusqu'à 48 h).

---

## 1. Comment ça marche, en six lignes

- **Un compte = un secret maître** (les 24 mots). Chaque machine du compte en garde
  une copie dans son keystore, scellée par la passphrase de *cette* machine, plus
  sa propre clé d'appareil.
- **Tes fichiers sont découpés, chiffrés, envoyés** à tes autres machines et aux
  machines d'autres comptes qui ont promis de la place (`pledge`). Un hôte stocke
  des blocs qu'il ne peut pas lire, et il est audité au hasard.
- **Le coordinateur (sur le Pi, port 9898)** est un annuaire et un casier : qui est
  quelle machine, où la joindre, le conteneur de récupération par passphrase. Il ne
  détient aucune clé. Sur un même réseau les machines se trouvent sans lui
  (découverte UDP 21037).
- **Plusieurs machines, un compte** : `login` (24 mots, ou `--from` le coordinateur
  avec la passphrase), puis `register`, `pledge`, `folder`, `daemon`. `device list`
  montre toutes les machines inscrites ; `device forget` en retire une, pour de bon.
- **Plusieurs comptes, une machine** : chaque compte est une *instance* nommée,
  avec son dossier (`~/.itsanas-NOM`), sa passphrase, son service
  (`itsanas@NOM` / tâche `ITSaNAS-NOM`) et son port, choisi automatiquement. Les
  instances partagent la découverte réseau.
- **Limite honnête** : une machine volée *avec* sa passphrase (fichier du daemon)
  donne tout le compte ; retirer l'appareil ne l'annule pas.

---

## 2. Préparation (~45 min)

### 2.1 Les machines et les rôles

| Machine | Compte `nicolas` (instance par défaut) | Compte `voisin` (instance nommée) | Rôle en plus |
| --- | --- | --- | --- |
| **W** laptop Windows | oui | facultatif (`-Instance voisin`) | la machine du quotidien : H se mesure ici |
| **P** Raspberry Pi | oui | **oui** (`--instance voisin`) | coordinateur |
| **V** VM Freebox | oui | **oui** (`--instance voisin`) | hôte toujours allumé, machine « détruite » en D |
| Android | facultatif | — | voir 5.3 |
| Mac | facultatif | — | voir 5.3 |

Pourquoi deux comptes : B, E, F et G demandent des machines du **même** compte
(`nicolas` sur les trois) ; C et le relais aveugle de E demandent un hôte d'un
**autre** compte (`voisin`) ; et `voisin` à côté de `nicolas` sur P et V est
exactement ton cas 1a. Adapte les noms à ce qui existe déjà sur la flotte
(`sigseg42`, `mandarine`…), mais garde cette structure.

### 2.2 Mettre tout le monde sur la même version

Tout sur `main` après la fusion de la PR « two accounts on one machine ».
Redémarrer par le service, jamais à la main (un daemon lancé à la main a caché
une vieille version pendant une semaine).

- P et V : `git -C <checkout> pull`, `sh install/linux.sh --source <checkout> --yes`,
  puis `systemctl --user daemon-reload && systemctl --user restart itsanas`.
- P, coordinateur : `coordinator.sh` **ne compile pas**, il installe un binaire.
  Donc d'abord `cargo build --release -p itsanas-coordinator` dans le checkout,
  puis `sudo sh install/coordinator.sh --binary target/release/itsanas-coordinator`
  et `sudo systemctl restart itsanas-coordinator`. **Obligatoire** : sans ça,
  `device list` retombe sur la liste partielle en disant que le coordinateur est
  plus ancien que le client.
- **Tous les nœuds d'une machine avant d'en ajouter un.** Un nœud d'une version
  ancienne prend le port de découverte pour lui seul ; un nouveau à côté tourne
  découverte coupée (sur Windows, erreur 10013 dans son journal).
- W : `install\windows.ps1 -Yes` puis `Stop-ScheduledTask ITSaNAS; Start-ScheduledTask ITSaNAS`.
  Toujours dû (HANDOVER §0) : la tâche a été créée en admin ; ta commande admin
  pour passer à `conhost --headless`.
- Vérifier : `git -C <checkout> log -1 --oneline` identique partout.

### 2.3 Installer la seconde instance (cas 1a)

Sur P puis V :

```sh
ITSANAS_PASSPHRASE='…' sh install/provision.sh --no-install --instance voisin \
  --username voisin --pledge 5G --folder ~/ITSaNAS-voisin \
  --coordinator <ip-du-pi>:9898 --coordinator-device <id>
```

(`--phrase-file` sur la seconde machine du compte, `--invite <code>` si le
coordinateur admet sur invitation.) Sur Windows, même chose avec `provision.ps1
-Instance voisin …`.

**Les nœuds qui existent déjà ne bougent pas.** Le port libre n'est choisi qu'à
la création (`init`/`login`). Si la VM a déjà deux nœuds sur 9797 (`voisin`,
`mandarine`), ou un nœud hors de `~/.itsanas*` que la détection ne voit pas,
donne-lui un port à la main : `ITSANAS_HOME=<son dossier> itsanas listen
0.0.0.0:9798` puis `itsanas register`, ou recrée-le en instance.

**Avant la matinée de test** : joue 2.2 et 2.3 à blanc sur la VM seule. Une heure
perdue là en épargne une quand les trois machines attendent.

**Vérifier, sur P et V** — tu notes le résultat en une ligne :

- `ITSANAS_HOME=~/.itsanas-voisin itsanas listen` ≠ celui de `itsanas listen`.
- `systemctl --user status itsanas itsanas@voisin` : les deux actifs.
- `journalctl --user-unit itsanas@voisin | grep found` : « found another user's
  device … » ; **jamais** « local discovery is off ».

### 2.4 État de départ

Sur chaque machine et chaque instance :
- `itsanas pledge` au moins 5G (un nœud à pledge 0 ne relaie rien ; le tour de
  synchronisation affiche alors `refused N offer(s): its pledge is full or zero`
  — si tu vois cette ligne pendant les tests, c'est ce prérequis qui manque) ;
- `itsanas status` ;
- `itsanas device list` (compte `nicolas`) : retire toute machine morte listée ;
- sur une machine `nicolas` : `itsanas register --recovery`. C'est *cette*
  passphrase qui servira en D.

Le kit tourne sous **Git Bash** sur Windows (`bash scripts/acceptance.sh …`).
Pour une instance nommée, préfixe `ITSANAS_HOME=~/.itsanas-voisin`.

---

## 3. Le MVP : tests A à M

W = laptop, P = Pi, V = VM, `~/ITSaNAS` = dossier synchronisé du compte `nicolas`.

### 3.0 Le fil : une histoire, pas une liste

Les lettres sont des propriétés à vérifier, pas un ordre de travail. Suis
l'histoire ci-dessous du haut vers le bas ; chaque étape dit quelles lettres
elle valide. Utilise des données **reconnaissables à l'œil** — une photo dont tu
te souviens, un document dont tu connais la première ligne — parce que tu vas
passer la journée à te demander « est-ce que c'est bien celui-là ».

| # | Sur | Ce que tu fais | Lettres |
| --- | --- | --- | --- |
| 1 | P | Compte `nicolas`. Tu déposes **une photo et trois PDF** dans `~/ITSaNAS` | — |
| 2 | V | Même compte. Tu dois **retrouver les quatre fichiers** | **B**, et **D** si tu es parti d'un dossier neuf |
| 3 | V | Tu **supprimes un PDF** et tu **ajoutes une deuxième photo** | — |
| 4 | P | La suppression et la nouvelle photo **sont arrivées** | **F** |
| 5 | P et V | Hors réseau des deux côtés, tu édites **le même fichier** différemment, puis tu reconnectes | **G** |
| 6 | W | Tu écris un fichier, tu attends deux tours, **tu éteins W**, puis V le récupère | **E** |
| 7 | W | Deuxième compte `voisin`. Il héberge les blocs de `nicolas` **sans pouvoir les lire** | **C** — *si C échoue, on arrête le projet* |
| 8 | P ou V | `itsanas status` : **où sont les copies**, combien sont confirmées | **L** |
| 9 | W | Tu **effaces le vault** de `voisin` à la main. P doit s'en apercevoir et replacer ailleurs | **K** |
| 10 | V | Deuxième instance pour `voisin` : elle récupère ses données, et **les deux instances ne se voient pas** | **M** |
| 11 | W | Une journée normale, daemon lancé | **H** |
| 12 | coord. | Coordinateur coupé, **deux fois** : tout à la maison, puis W en partage de connexion | **I** |
| 13 | les trois | Redémarrages, dont le Pi par coupure de courant | **J** |

**Ne commence pas par 10 Go de vidéos.** Les chiffres sont dans `MVP.md` §6 :
27,2 Mio/s d'archivage sur le laptop et **14,7 millions de fichiers par
téraoctet**, soit ~150 000 fichiers pour 10 Go, à pousser ensuite vers deux
autres machines — dont la carte SD du Pi, et ce projet en a déjà tué une. Les
*pack files* sont le correctif décidé et **ne sont pas construits**. Quelques
centaines de Mo suffisent pour tout ce qui est ci-dessus. Les 10 Go sont un test
de débit, à faire exprès, en sachant que c'est le débit qu'on mesure.

| # | Où | Quoi faire | Réussi si |
| --- | --- | --- | --- |
| **A** | partout | aucune phase : les installations de la partie 2 se sont faites sans éditer un fichier ni taper une adresse de pair | vrai sur les trois |
| **B** | W puis P, V | W : `B write ~/ITSaNAS` → nom + sha256. Sur P et V, dans la minute où tous sont éveillés : `B check ~/ITSaNAS <nom> <sha>` | PASS sur P et V |
| **C** | W puis instance `voisin` de V | W : `C plant ~/ITSaNAS` → canari. Attendre deux tours. Sur V : `C scan <canari> ~/.itsanas-voisin` | PASS. **Si C échoue, on arrête le projet.** |
| **D** | V, dossier neuf | `ITSANAS_HOME=~/itsanas-d itsanas login --username nicolas --from <ip-du-pi>:9898 --device <id>` (passphrase seule, **pas les 24 mots**), puis `… register`, `… pledge 1G`, `… sync` (sans adresse : il demande au coordinateur les machines du compte allumées ; si aucune ne l'est, `… daemon` quelques minutes), puis `ITSANAS_HOME=~/itsanas-d bash scripts/acceptance.sh D check <chemin> <sha>` | PASS sans avoir tapé d'adresse de pair |
| **E** | W, puis V ; seules les instances `voisin` servent de relais | **Arrêter `itsanas` (compte `nicolas`) sur P *et* sur V** ; garder `itsanas@voisin` sur les deux : ce sont les seuls relais, et ils ne peuvent pas lire. Sinon V récupère le fichier chez `nicolas` sur P, qui le lit, et E passe sans avoir rien prouvé. W : `E write ~/ITSaNAS`, attendre deux tours (`itsanas status` sur W : les blocs sont ailleurs), **puis éteindre W**. Relancer `itsanas` sur V seulement : `E check ~/ITSaNAS <nom> <sha>`. Puis relancer `itsanas` sur P | PASS alors que V n'a pu joindre aucune machine de son compte |
| **F** | W, puis V | `itsanas` de V arrêté. W : `F delete ~/ITSaNAS <nom>`, un tour, W éteinte. V relancé : `F check ~/ITSaNAS <nom>`, puis **encore après un tour** | deux PASS ; rien d'autre n'a disparu |
| **G** | W et V hors réseau | W : wifi coupé. V : `itsanas` arrêté. Sur chacune : `G edit ~/ITSaNAS <nom> <tag-différent>`. Reconnecter, deux tours, `G check ~/ITSaNAS <nom>` sur **les deux** | PASS des deux côtés et **même empreinte** |
| **H** | W surtout | **W** : `powershell -ExecutionPolicy Bypass -File scripts\acceptance.ps1 H schedule`, une journée normale, puis `… H report` (CPU, mémoire, écritures, et un rapport batterie `powercfg` à lire à côté), puis `… H sleep` **dans un PowerShell administrateur** (le daemon empêche-t-il la veille, a-t-il réveillé la machine) ; `… H unschedule` pour arrêter. **P et V** : `H sample` toutes les 5 min pendant 24 h, puis `H report` | **`H report` et `H sleep` PASS, et le rapport batterie lu par toi** — `H report` seul ne couvre que CPU et mémoire. Laisse le laptop se mettre en veille au moins une fois pendant la journée, daemon lancé : sans veille, `H sleep` refuse de conclure |
| **I** | coordinateur coupé | `sudo systemctl stop itsanas-coordinator` (48 h visées ; note la durée réelle). Pendant la coupure, écrire un fichier sur W. Sur V : `journalctl --user-unit itsanas --since "<début>" > /tmp/daemon.log` puis `I check /tmp/daemon.log`. `itsanas status` doit dire ce qui est dégradé. Relancer | PASS, et le fichier est arrivé |
| **J** | les trois | Sur chaque : `J count ~/ITSaNAS`. Redémarrer les trois dans n'importe quel ordre, **dont le Pi par coupure de courant pendant un gros `itsanas put`**. Daemon arrêté : `J check ~/ITSaNAS <nombre donné par J count>` | PASS partout, aucun `doctor --repair` |
| **K** | W (hôte), puis P | Pas de phase du kit. Sur W, **efface le dossier `vault` de `voisin`** (pas `store/blobs` : ça, ce sont ses propres blocs). **Le daemon de P doit tourner** — `itsanas sync` ne fait *jamais* d'audit, seule la boucle du daemon lance les défis ; un `sync` à la main ne montrera rien et tu conclurais à tort que la sanction ne marche pas. Puis `itsanas status` sur P | Le daemon de P affiche `FAILED n of m storage challenges`, et `itsanas status` gagne une section `peers that have failed a storage challenge` qui **nomme la machine**. Rien n'est perdu ; trois échecs consécutifs et W ne reçoit plus rien de neuf. *Ne te fie pas au compteur `placements` : en test automatisé il n'a pas bougé alors que le défi avait bien échoué. Le nom du pair est la preuve.* *Le vault est un dossier ordinaire : l'effacer ne demande aucun privilège et rien ne prévient — c'est connu, ce n'est pas le résultat du test* |
| **L** | n'importe laquelle | Ne tape aucune commande. Regarde ce que le logiciel te dit de lui-même | **Échoue aujourd'hui, c'est attendu.** `itsanas status` sait déjà tout dire (`the promise`, `spreading off`, `headroom`, `unconfirmed`) mais il faut le demander, **et il faut la passphrase**. Aucune alerte n'existe : `ARCHITECTURE.md` §7 est une spec vide. Note ce que tu aurais voulu voir et où |
| **M** | V, deux instances | Suis la partie 4. Sur chaque instance : `itsanas status` et le contenu du dossier | Chacune ne montre **que** son compte ; celle qui héberge l'autre ne peut pas lire ; arrêter l'une laisse l'autre synchroniser |

**L'expérience à dix secondes**, pendant J sur le Pi : `itsanas put big.bin
<quelques centaines de Mo>`, débrancher au milieu, redémarrer, `itsanas doctor
--deep`. Deux fois. Un fichier qui ne vérifie pas → le flush sert ; seulement des
blocs orphelins → il ne sert à rien et l'écriture peut aller deux fois plus vite.

---

## 4. Plusieurs comptes sur une machine (1a)

Sur P, avec `nicolas` et `voisin` qui tournent :

| # | Quoi faire | Réussi si |
| --- | --- | --- |
| **1a-1** | 2.3 fait sans erreur | deux services actifs, deux ports différents |
| **1a-2** | redémarrer P ; ne rien toucher | les deux instances reviennent seules |
| **1a-3** | un fichier dans `~/ITSaNAS` (`nicolas`) et un autre dans `~/ITSaNAS-voisin` | chacun n'apparaît que dans son compte, sur toutes ses machines |
| **1a-4** | `C scan <canari de nicolas> ~/.itsanas-voisin` sur P | PASS : deux comptes sur un même disque restent aveugles l'un à l'autre |
| **1a-5** | `sh install/clean.sh --instance voisin` (dry run), puis `--yes` | seule l'instance disparaît ; `itsanas` (`nicolas`) tourne toujours |

---

## 5. Un compte, plusieurs machines (1b)

### 5.1 Retirer une machine, changer une passphrase

Avec la machine D (`~/itsanas-d` sur V), avant de la supprimer :

1. Sur W : `itsanas device list` la montre (« heard from … »).
2. `itsanas device forget <12 premiers caractères>`.
3. Sur V : `ITSANAS_HOME=~/itsanas-d itsanas register` doit **refuser** (« withdrawn from
   this account »).
4. `ITSANAS_HOME=~/itsanas-d itsanas passphrase` : la nouvelle ouvre (`whoami`), l'ancienne non.
5. `rm -rf ~/itsanas-d`.

### 5.2 Une machine muette

Éteindre V une semaine n'est pas raisonnable ; à la place, sur W :
`itsanas device list` doit lister *aussi* les machines éteintes, avec depuis
combien de temps elles se taisent. Note ce que tu vois pour chacune.

### 5.3 Android et Mac (facultatif, et dit honnêtement)

- **Android** : la seule APK publiée (v0.1.0, signature de debug) est vieille de
  plusieurs semaines de corrections et n'a jamais tourné que sur émulateur. Il faut
  en construire une neuve (`scripts/build-apk.sh`, SDK et NDK requis). Test :
  restaurer `nicolas` avec les 24 mots, ajouter W comme machine, synchroniser,
  ouvrir un fichier. **Pas de dossier qui se synchronise tout seul** : l'app tient
  des fichiers, elle ne surveille pas un répertoire.
- **Mac** : pas de binaire ; `sh install/macos.sh` depuis un checkout (compile, Apple
  silicon validé en CI seulement), puis les mêmes commandes qu'un Linux. Une seule
  instance (le service launchd n'a pas de variante nommée).

---

## 6. Les mesures qui parlent d'échelle

Elles ne décident pas du MVP. Elles disent si la suite a un sens. Note chaque
chiffre avec la machine.

| Mesure | Comment | Pourquoi elle compte |
| --- | --- | --- |
| Débit local | `itsanas bench --size 1G` sur W, P, V | combien d'heures pour charger 1 To (connu : ~27 Mio/s laptop, ~54 VM) |
| Débit réseau | un fichier de 1 Go dans `~/ITSaNAS` sur W, chronométrer jusqu'à `B check` PASS sur P puis sur V | le vrai goulot pour quiconque n'est pas sur ton réseau |
| Temps de restauration | en D, chronométrer `sync` pour tout le compte, et noter sa taille (`itsanas status`) | « je perds mon laptop, combien de temps avant de retravailler » |
| Écritures au repos | `H report` sur P et V | un SD de Pi qui meurt en un an disqualifie un nœud grand public |
| Fichiers par Go | `find ~/.itsanas/store -type f \| wc -l` puis diviser par la taille | un fichier par bloc : 14,7 millions par To, mesuré ; au-delà, les pack files manquent |
| Coût d'un hôte | sur l'instance `voisin` : `itsanas status` (ce qu'elle héberge), CPU/RAM pendant un tour | ce que ça coûte d'héberger les autres |

---

## 7. Ce que ce protocole ne peut PAS montrer

Un vert partout dira « ça marche pour une personne ». Il ne dira pas « viable comme
Storj ». Voici pourquoi, point par point, pour que personne ne lise un succès de
travers.

| Inconnu | Pourquoi tes machines ne le voient pas | État dans le code |
| --- | --- | --- |
| **Des inconnus hostiles, nombreux** | tous les nœuds sont à toi ; personne ne triche | audits aléatoires et red-team en labo ; jamais contre un vrai tricheur |
| **NAT et Internet** | P et V sont chez toi ou à IP publique | **pas de traversée de NAT** ; une machine derrière un NAT se joint seulement en sortant |
| **Bande passante** | réseau local rapide | aucune comptabilité de débit (HANDOVER §9) |
| **Le téraoctet** | tes comptes pèsent des Go | un fichier par bloc ; pack files décidés, pas construits ; l'audit couvre 1 To en ~10 ans |
| **L'économie** | tu ne peux pas te voler toi-même | le partage 30/70 n'est appliqué que localement ; un client modifié stocke sans donner (HANDOVER §8.1 b-c) |
| **L'usage déclaré** | — | l'usage est auto-déclaré ; les hôtes ne le vérifient pas |
| **Troncature de l'historique** | — | un hôte peut servir un préfixe cohérent du journal (non détecté) |
| **Coût du stockage** | — | réplication ×3 ; Storj fait du codage à effacement (moins de surcoût pour la même durabilité) et paie ses opérateurs. Ici, troc sans paiement |
| **Le coordinateur** | un seul, chez toi | point central pour trouver et récupérer ; un réseau ouvert en demande plusieurs, ou une DHT (DESIGN §8) |

**Ce qui distingue vraiment le projet**, et que ce protocole peut confirmer : C et
1a-4 (l'hôte est aveugle, vérifiable en une commande), E (relais aveugle entre
machines qui ne se voient jamais), et l'absence de loyer. Si ces trois tiennent sur
tes machines, la question devient : *quelqu'un d'autre voudrait-il héberger tes
blocs contre les siens ?* — et elle se teste avec une deuxième personne, pas avec
une quatrième machine.

---

## 8. Décider

1. **Le MVP** : la règle de MVP.md §4, telle quelle. C rouge → stop. A, B, D, F ou J
   rouge → démo, pas MVP. E, G ou I rouge → le design distribué est faux quelque
   part. H rouge → on corrige avant tout le reste.
2. **1a et 1b** : un rouge est un bug à corriger avant d'inviter qui que ce soit.
3. **La suite « à la Storj »**, seulement si le MVP passe. Les chiffres de la
   partie 6 qui justifieraient d'y aller : restauration de ton compte réel en
   moins d'une nuit, écritures au repos compatibles avec une carte SD, CPU/batterie
   invisibles sur W. Ensuite, dans l'ordre : une deuxième personne (appliquer le
   partage chez l'hôte, HANDOVER §8.1 c), les pack files, la traversée de NAT.

## 9. Ce que tu me renvoies

1. `~/.itsanas-receipts/acceptance.txt` de chaque machine et de chaque instance.
2. Une ligne par constat manuel (2.3, 1a, 5.1, 5.2, H Windows) avec les chiffres.
3. Le tableau de la partie 6 rempli.
4. Pour chaque FAIL : la sortie complète de la commande, `itsanas status`, et le
   journal du daemon.
