# Briefing : tester le MVP à la main (Windows, VM, Pi)

Pour Nicolas. Ce que tu fais de tes mains pour que le verdict de
[MVP.md](MVP.md) §4 soit pris sur la flotte réelle et non au labo. Chaque test
renvoie `PASS` ou `FAIL` avec les chiffres via `scripts/acceptance.sh` et
s'ajoute à `~/.itsanas-receipts/acceptance.txt`. **Tu colles ces fichiers, pas
une impression.** Un test qui demande un contournement, un indice ou un second
essai est un échec (MVP.md §3).

## 1. Comment ça marche, en cinq lignes

- **Un compte = un secret maître** (les 24 mots). Chaque machine du compte en
  garde une copie dans son keystore, scellée par la passphrase de *cette*
  machine, plus sa propre clé d'appareil.
- **Tes fichiers sont découpés, chiffrés et envoyés** à tes autres machines et
  aux machines d'autres comptes qui ont promis de la place (`pledge`). Un hôte
  stocke des blocs qu'il ne peut pas lire ; il est audité au hasard.
- **Le coordinateur (sur le Pi, port 9898)** n'est qu'un annuaire et un casier :
  qui est quelle machine, où la joindre, et le conteneur de récupération par
  passphrase (s'il a été déposé). Il ne détient aucune clé. Sur un même réseau
  les machines se trouvent sans lui (découverte UDP 21037).
- **Ajouter une machine au compte** : `login` (24 mots, ou `--from` le
  coordinateur avec la passphrase), puis `register`, `pledge`, `folder`,
  `daemon`. Depuis cette PR, `login --from` garde le coordinateur, et
  `register` marche donc directement après.
- **Retirer une machine** : `itsanas device list` (toutes les machines inscrites,
  même muettes depuis des semaines) puis `itsanas device forget <id court>`
  depuis une *autre* machine du compte. Le retrait est **définitif** pour cet
  identifiant ; pour réutiliser la machine, on supprime son dossier de nœud et on
  refait `login`. **Limite honnête** : une machine volée *avec* sa passphrase
  (fichier du daemon) donne tout le compte ; le retrait ne l'annule pas.

## 2. Préparation (une fois, ~20 min)

1. **Mettre les trois machines sur `main` après la fusion de cette PR**, et
   redémarrer par le service, jamais à la main (HANDOVER §0 : un daemon lancé à la
   main a caché une vieille version pendant une semaine).
   - Pi et VM : réinstaller depuis le checkout à jour, puis
     `systemctl --user restart itsanas`.
   - Pi, coordinateur : réinstaller, puis `sudo systemctl restart itsanas-coordinator`.
     **Obligatoire** : sans ça, `device list` affiche seulement les machines vues
     cette semaine et le dit.
   - Windows : réinstaller, puis `Stop-ScheduledTask ITSaNAS; Start-ScheduledTask ITSaNAS`.
   - Vérifier : `git -C <checkout> log -1 --oneline` identique sur les trois.
2. **Toujours dû sur le laptop** (HANDOVER §0) : la tâche planifiée a été créée
   en admin ; passer son action à `conhost --headless` demande ta commande admin.
3. **Chaque machine promet de la place** : `itsanas pledge 10G` au minimum. Un
   nœud à pledge 0 ne relaie rien et `sync` affiche alors `sent 0 B`, comme s'il
   n'y avait rien à envoyer.
4. **État de départ**, sur chaque machine : `itsanas status`, puis
   `itsanas device list`. Retire (`device forget`) toute machine morte listée. Note
   quel compte tourne où (laptop, Pi, VM : `nicolas`, `voisin`, `mandarine`,
   `sigseg42`…), il en faut deux différents pour C.
5. **Conteneur de récupération** : sur une machine du compte testé,
   `itsanas register --recovery` (il demande la passphrase ; c'est *celle-là* qui
   servira en D).

**Piège à vérifier avant B, E, F, G : ils demandent des machines du *même*
compte.** Le dossier synchronisé ne reçoit que les fichiers de son propre compte ;
un hôte d'un autre compte garde des blocs illisibles et rien dans `~/ITSaNAS`.
D'après le handover, la flotte a eu `sigseg42` sur le laptop, `nicolas` sur le
Pi et `voisin`/`mandarine` sur la VM. Si c'est encore le cas, ajoute sur P et V
un nœud du compte du laptop (`ITSANAS_HOME=~/itsanas-w itsanas login --username
<compte-du-laptop> --from <ip-du-pi>:9898 --device <id>`, puis `register`,
`pledge`, `folder`, `daemon`) avant ces quatre tests, et fais tourner le kit
avec ce `ITSANAS_HOME`. Garde les nœuds des autres comptes : ce sont eux qui
servent C et le relais aveugle de E.

Sur Windows, le kit tourne sous **Git Bash** (`bash scripts/acceptance.sh …`).
Sur la VM, deux nœuds se partagent le port de découverte : le second ne trouve
ses pairs que via le coordinateur ou `itsanas peer add`.

## 3. Les tests, dans l'ordre qui coûte le moins

Notation : **W** = laptop Windows, **P** = Pi, **V** = VM Freebox. `~/ITSaNAS` =
le dossier synchronisé de la machine.

| # | Où | Quoi faire | Réussi si |
| --- | --- | --- | --- |
| **B** | W puis P, V | W : `bash scripts/acceptance.sh B write ~/ITSaNAS` → nom + sha256. Sur P et V, dans la minute où tous sont éveillés : `B check ~/ITSaNAS <nom> <sha>` | PASS sur P et V |
| **C** | W puis l'hôte d'un **autre** compte | W : `C plant ~/ITSaNAS` → canari. Attendre un tour (5 min). Sur la machine d'un autre compte qui héberge W : `C scan <canari> ~/.itsanas` | PASS (le contrôle interne prouve que la recherche marche). **Si C échoue, on arrête le projet.** |
| **D** | V, dossier neuf | `ITSANAS_HOME=~/itsanas-d itsanas login --username <compte> --from <ip-du-pi>:9898 --device <id du coordinateur>` (passphrase seule, **pas les 24 mots**), puis `ITSANAS_HOME=~/itsanas-d itsanas register`, `… pledge 1G`, `… sync` (sans adresse : il interroge le coordinateur et ne joint que les machines **de ce compte** qui sont allumées — si aucune ne l'est, lance plutôt `… daemon` quelques minutes, qui trouve aussi les hôtes du réseau local, puis arrête-le), puis `ITSANAS_HOME=~/itsanas-d bash scripts/acceptance.sh D check <chemin> <sha>` d'un fichier connu | PASS, sans avoir tapé une adresse de pair |
| **D′ comptes** | W et `~/itsanas-d` | Sur W : `itsanas device list` montre la machine D (« heard from … »). `itsanas device forget <12 premiers caractères>`. Sur V : `ITSANAS_HOME=~/itsanas-d itsanas register` doit **refuser** (« withdrawn from this account »). Puis `ITSANAS_HOME=~/itsanas-d itsanas passphrase` : la nouvelle ouvre (`whoami`), l'ancienne non. Enfin `rm -rf ~/itsanas-d` | les quatre constats, notés à la main |
| **E** | V off, W, puis V | Éteindre V (ou `systemctl --user stop itsanas`). W : `E write ~/ITSaNAS`, attendre un tour complet avec P éveillé (sur W, `itsanas status` doit dire que les blocs sont ailleurs), **puis éteindre W**. Rallumer V : `E check ~/ITSaNAS <nom> <sha>` | PASS alors que W et V ne se sont jamais vus éveillés |
| **F** | W, puis V | V éteinte. W : `F delete ~/ITSaNAS <nom>`, un tour, W éteinte. V rallumée : `F check ~/ITSaNAS <nom>`, puis **encore après un tour** | deux PASS ; rien d'autre n'a disparu |
| **G** | W et V hors réseau | W : wifi coupé. V : daemon arrêté. Sur chacune : `G edit ~/ITSaNAS <nom> <tag-différent>`. Reconnecter les deux, attendre deux tours, puis `G check ~/ITSaNAS <nom>` sur **les deux** | PASS des deux côtés et **même empreinte** |
| **I** | P coordinateur coupé | `sudo systemctl stop itsanas-coordinator` (48 h visées ; note la durée réelle). Pendant la coupure, écrire un fichier sur W. Sur V : `journalctl --user -u itsanas --since "<début>" > /tmp/daemon.log` puis `I check /tmp/daemon.log`. `itsanas status` doit dire ce qui est dégradé. Relancer le coordinateur | PASS, et le fichier est arrivé |
| **J** | les trois | Sur chaque : `J count ~/ITSaNAS`. Redémarrer les trois dans n'importe quel ordre, **dont le Pi par coupure de courant pendant un gros `itsanas put`**. Daemon arrêté, `J check ~/ITSaNAS <nombre donné par J count>` | PASS partout, aucun `doctor --repair` |
| **H** | W surtout | P et V : `H sample` toutes les 5 min pendant 24 h (cron/timer), puis `H report`. **W, que le kit ne mesure pas** : `powercfg /batteryreport` avant/après une journée normale, gestionnaire des tâches (CPU au repos, mémoire de `itsanas.exe`), `powercfg /requests` (le daemon ne doit pas empêcher la veille) | CPU au repos négligeable, < 200 Mo, batterie inchangée, la veille marche |

**A** n'a pas de phase : il est réussi si D′ et les installations se sont fait
sans éditer un fichier ni taper une adresse de pair.

**L'expérience à dix secondes (MVP.md §5, fin)**, pendant J sur le Pi :
`itsanas put big.bin <quelques centaines de Mo>`, débrancher au milieu,
redémarrer, `itsanas doctor --deep`. Deux fois. Dis-moi s'il signale un fichier
qui ne vérifie pas (le flush sert) ou seulement des blocs orphelins (il ne sert
à rien et l'écriture peut aller deux fois plus vite).

## 4. Ce que tu me renvoies

1. `~/.itsanas-receipts/acceptance.txt` des trois machines.
2. Les constats de D′ et de H sur Windows, en une ligne chacun avec les chiffres.
3. Pour chaque FAIL : la sortie complète de la commande et `itsanas status`.

Le verdict se prend avec la règle écrite *avant* (MVP.md §4) : C rouge → stop ;
A, B, D, F ou J rouge → démo, pas MVP ; E, G ou I rouge → le design distribué
est faux quelque part ; H rouge → on corrige avant tout le reste.
