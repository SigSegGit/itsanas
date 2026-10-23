# Tester ITSaNAS avec Nicolas : la page du partenaire

Une page, trois essais, et chaque essai se termine par une ligne qui dit
**PASS** ou **FAIL**. Rien à interpréter, aucun journal à lire : si une ligne
dit FAIL, envoie-la telle quelle à Nicolas, c'est tout ce qu'il lui faut.

Tout ce qui peut se vérifier sans toi est vérifié chaque nuit sur les
machines de Nicolas ([FLEET-CI.md](FLEET-CI.md)). Ce qui reste ici, c'est ce
qu'aucun robot ne sait faire : ta machine, chez toi, sur ton réseau.

**macOS, Linux et Windows.** Les commandes sont écrites pour un Terminal
(Mac, Linux) ; sous Windows, la version PowerShell est en fin de page.

## Avant de commencer (une fois)

ITSaNAS est installé et tourne (Nicolas t'a guidé pour ça). Récupère le kit
d'essai, dans un Terminal :

```
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/scripts/acceptance.sh -o ~/itsanas-essai.sh
```

Ton dossier synchronisé est `~/ITSaNAS` (ou `~/ITSaNAS-<nom>` si Nicolas t'a
créé un compte nommé : remplace-le dans les commandes ci-dessous).

## Essai 1 : un fichier de chez toi arrive chez Nicolas

Chez toi :

```
bash ~/itsanas-essai.sh B write ~/ITSaNAS
```

La ligne se termine par `code xxxx-xxxx`. **Dis ce code à Nicolas** (huit
caractères, chiffres et lettres de a à f ; pas besoin du nom du fichier).
Chez lui, il tape `B check ~/ITSaNAS <ton code>` et lit PASS ou FAIL. Un
mauvais code donne FAIL, jamais un faux PASS.

## Essai 2 : un fichier de chez Nicolas arrive chez toi

Nicolas fait `B write` chez lui et te dit son code. Chez toi, attends une
minute ou deux, puis :

```
bash ~/itsanas-essai.sh B check ~/ITSaNAS <son code>
```

FAIL tout de suite ? Attends encore quelques minutes et relance : FAIL veut
dire « pas encore là, ou arrivé différent ». S'il dit encore FAIL au bout de
dix minutes, envoie la ligne.

## Essai 3 : ce que tu effaces disparaît aussi chez Nicolas

Nicolas te donne un nom de fichier (il commence par `acceptance-`). Chez toi :

```
bash ~/itsanas-essai.sh F delete ~/ITSaNAS <le nom>
```

Puis c'est Nicolas qui vérifie chez lui (`F check`).

## Ce qui compte aussi, et que seul toi peux dire

- Ce que tu as trouvé difficile, dans tes mots, même si tout a dit PASS.
- Une capture d'écran de l'icône ITSaNAS (barre des menus) et de la page
  Réglages, si quelque chose t'y semble faux ou pas clair.

Toutes les lignes PASS et FAIL sont aussi gardées dans
`~/.itsanas-receipts/acceptance.txt` : tu peux envoyer ce fichier à la fin.

## Sous Windows

Dans PowerShell (menu Démarrer, « PowerShell »), une fois :

```
irm https://raw.githubusercontent.com/SigSegGit/itsanas/main/scripts/acceptance.ps1 -OutFile $HOME\itsanas-essai.ps1
```

Puis les mêmes essais, avec `powershell -ExecutionPolicy Bypass -File
$HOME\itsanas-essai.ps1` à la place de `bash ~/itsanas-essai.sh`, et
`$HOME\ITSaNAS` à la place de `~/ITSaNAS` :

```
powershell -ExecutionPolicy Bypass -File $HOME\itsanas-essai.ps1 B write $HOME\ITSaNAS
powershell -ExecutionPolicy Bypass -File $HOME\itsanas-essai.ps1 B check $HOME\ITSaNAS <code>
powershell -ExecutionPolicy Bypass -File $HOME\itsanas-essai.ps1 F delete $HOME\ITSaNAS <nom>
```

Les codes sont les mêmes d'un système à l'autre : un fichier écrit sur un Mac
se vérifie sous Windows, et inversement.
