// Translations of the setup and settings page, keyed by the English text in
// index.html and app.js. A key missing here shows in English, and
// `every_text_of_the_page_has_a_french_translation` (web/tests.rs) fails.
// To add a language: a new object beside `fr`, and an <option> in #lang.
'use strict';

window.I18N = {
  fr: {
    'Skip to the current step': 'Aller à l’étape en cours',
    'Set up this machine': 'Installer cette machine',
    'Language': 'Langue',
    'Steps': 'Étapes',
    'This page needs JavaScript. Close it and run': 'Cette page a besoin de JavaScript. Fermez-la et lancez',
    'in a terminal instead.': 'dans un terminal à la place.',
    'Welcome': 'Bienvenue',
    "ITSaNAS keeps a folder in sync between your machines. Your files are locked with a key only you have, and copies are kept on other members' computers in case yours breaks.":
      'ITSaNAS synchronise un dossier entre vos machines. Vos fichiers sont verrouillés par une clé que vous seul avez, et des copies sont gardées sur les ordinateurs d’autres membres au cas où le vôtre tomberait en panne.',
    'This takes about five minutes. Each step is saved: if something stops, run setup again and it picks up where it left off.':
      'Comptez environ cinq minutes. Chaque étape est enregistrée : si quelque chose s’arrête, relancez l’installation et elle reprend là où elle en était.',
    'Start': 'Commencer',
    'This machine': 'Cette machine',
    'Check that this is the machine you want to set up.': 'Vérifiez que c’est bien la machine que vous voulez installer.',
    'Its ITSaNAS data goes in': 'Ses données ITSaNAS vont dans',
    'Name of this setup': 'Nom de cette installation',
    'Back': 'Retour',
    'Next': 'Suivant',
    'Your account': 'Votre compte',
    'Is this your first machine with ITSaNAS?': 'Est-ce votre première machine avec ITSaNAS ?',
    'Account': 'Compte',
    'Yes, create a new account': 'Oui, créer un nouveau compte',
    'No, I already have an account': 'Non, j’ai déjà un compte',
    'Your username': 'Votre nom d’utilisateur',
    'Letters and digits. Use the same one on all your machines.': 'Lettres et chiffres. Le même sur toutes vos machines.',
    "My account's recovery copy is kept by the network (I chose it on my first machine)":
      'La copie de secours de mon compte est gardée par le réseau (je l’ai choisi sur ma première machine)',
    'Otherwise a separate window asks for your 24 recovery words.': 'Sinon, une fenêtre séparée demande vos 24 mots de récupération.',
    'Your secret': 'Votre secret',
    'Never type your recovery words into a web page — not even this one.': 'Ne tapez jamais vos mots de récupération dans une page web — pas même celle-ci.',
    'Nothing opens yet. After the last question, when you press': 'Rien ne s’ouvre pour l’instant. Après la dernière question, quand vous appuierez sur',
    'Start setting up': 'Lancer l’installation',
    ', a separate ITSaNAS window will open for your secret — never this page.': ', une fenêtre ITSaNAS séparée s’ouvrira pour votre secret — jamais cette page.',
    'A new account gets 24 recovery words: the only way back into your files if this machine is lost. Keep them in a password manager (you may copy them or save them to a file) or on paper. The window then asks three of them back.':
      'Un nouveau compte reçoit 24 mots de récupération : le seul moyen de retrouver vos fichiers si cette machine est perdue. Gardez-les dans un gestionnaire de mots de passe (vous pouvez les copier ou les enregistrer dans un fichier) ou sur papier. La fenêtre vous en redemande ensuite trois.',
    'An existing account: the window asks for your 24 words, unless the network keeps your recovery copy.':
      'Un compte existant : la fenêtre demande vos 24 mots, sauf si le réseau garde votre copie de secours.',
    'Then it asks for a passphrase that unlocks this machine.': 'Puis elle demande une phrase de passe qui déverrouille cette machine.',
    'Joining the network': 'Rejoindre le réseau',
    'ITSaNAS joins the network by itself: there is nothing to type here.':
      'ITSaNAS rejoint le réseau tout seul : il n’y a rien à taper ici.',
    'Invitation code (only for a private network that asks for one)': 'Code d’invitation (seulement pour un réseau privé qui en demande un)',
    'Advanced': 'Avancé',
    'Coordinator address': 'Adresse du coordinateur',
    'Leave it as it is unless you run your own network. Empty: only machines on this home network find each other.':
      'Laissez-la telle quelle sauf si vous avez votre propre réseau. Vide : seules les machines de ce réseau local se trouvent.',
    'Your folder': 'Votre dossier',
    'Choose the folder to sync on this machine. On your other machines it can be somewhere else: what is synced is what it holds.':
      'Choisissez le dossier à synchroniser sur cette machine. Sur vos autres machines, il peut être ailleurs : c’est son contenu qui est synchronisé.',
    'Folder': 'Dossier',
    'Browse…': 'Parcourir…',
    'It is created if it does not exist. Files you put in it are synced.': 'Il est créé s’il n’existe pas. Les fichiers que vous y mettez sont synchronisés.',
    'Start ITSaNAS when I log in, with an icon near the clock (recommended)':
      'Démarrer ITSaNAS à l’ouverture de session, avec une icône près de l’horloge (recommandé)',
    'Space you offer': 'Espace offert',
    'Choose how much of this disk other members may use for their copies. It is what earns room for your own files on theirs.':
      'Choisissez combien de ce disque les autres membres peuvent utiliser pour leurs copies. C’est ce qui vous donne de la place pour vos fichiers chez eux.',
    'Space to offer, in GB': 'Espace à offrir, en Go',
    "You can change it later. The disk is the one that holds this machine's ITSaNAS data.":
      'Vous pourrez le changer plus tard. Le disque est celui qui contient les données ITSaNAS de cette machine.',
    'Updates': 'Mises à jour',
    'New versions of ITSaNAS are signed by its author. This machine installs only a version with that signature.':
      'Les nouvelles versions d’ITSaNAS sont signées par son auteur. Cette machine n’installe qu’une version portant cette signature.',
    'When a new version is out': 'Quand une nouvelle version sort',
    'Install it by itself (once a day, it restarts in a few seconds)': 'L’installer toute seule (une fois par jour, redémarrage en quelques secondes)',
    'Tell me, and I install it (': 'Me prévenir, et je l’installe (',
    ')': ')',
    'Never look': 'Ne jamais vérifier',
    'Ready': 'Prêt',
    'Press Start. ITSaNAS then checks the connection, starts in the background and checks that it all works.':
      'Appuyez sur Lancer. ITSaNAS vérifie alors la connexion, démarre en arrière-plan et vérifie que tout fonctionne.',
    'A separate ITSaNAS window has opened in front of this page.': 'Une fenêtre ITSaNAS séparée s’est ouverte devant cette page.',
    'If you cannot see it, look for ITSaNAS in your taskbar / Dock.': 'Si vous ne la voyez pas, cherchez ITSaNAS dans la barre des tâches / le Dock.',
    'Details': 'Détails',
    'Final check': 'Vérification finale',
    'What to do:': 'Que faire :',
    'Open my folder': 'Ouvrir mon dossier',
    "Add it to the file manager's favourites": 'L’ajouter aux favoris de l’explorateur',
    'Try again': 'Réessayer',
    'Close': 'Fermer',
    'Settings': 'Réglages',
    'Reading…': 'Lecture…',
    'Syncing': 'Synchronisation',
    'Pause': 'Pause',
    'until I resume': 'jusqu’à ce que je reprenne',
    'Resume': 'Reprendre',
    'Sync now': 'Synchroniser maintenant',
    'Check for changes': 'Vérifier les changements',
    'automatically': 'automatiquement',
    'every 5 minutes': 'toutes les 5 minutes',
    'every 15 minutes': 'toutes les 15 minutes',
    'every hour': 'toutes les heures',
    'every 6 hours': 'toutes les 6 heures',
    'once a day': 'une fois par jour',
    'Save': 'Enregistrer',
    'Changing these restarts ITSaNAS (a few seconds).': 'Les modifier redémarre ITSaNAS (quelques secondes).',
    'Space offered to others, in GB': 'Espace offert aux autres, en Go',
    'Open': 'Ouvrir',
    'install it by itself': 'l’installer toute seule',
    'tell me': 'me prévenir',
    'never look': 'ne jamais vérifier',
    'Invitation code, if the coordinator asks for one': 'Code d’invitation, si le coordinateur en demande un',
    'Apply': 'Appliquer',
    'Sign out': 'Se déconnecter',
    'Stops ITSaNAS on this machine and forgets its passphrase. Your files and keys stay on this disk.':
      'Arrête ITSaNAS sur cette machine et oublie sa phrase de passe. Vos fichiers et vos clés restent sur ce disque.',
    'This page is closed': 'Cette page est fermée',
    'You can close this tab. The icon near the clock opens Settings again (or run': 'Vous pouvez fermer cet onglet. L’icône près de l’horloge rouvre les Réglages (ou lancez',
    ').': ').',
    'This username is already taken. Choose another.': 'Ce nom d’utilisateur est déjà pris. Choisissez-en un autre.',
    'Account found.': 'Compte trouvé.',
    'This username is free.': 'Ce nom d’utilisateur est libre.',
    'No account has this name on the network.': 'Aucun compte ne porte ce nom sur le réseau.',
    'The network cannot be asked right now; it is checked again before anything is made.':
      'Le réseau ne répond pas pour l’instant ; ce sera revérifié avant de créer quoi que ce soit.',
    'You offer': 'Vous offrez',
    'you get about': 'vous obtenez environ',
    'for your own files on the network.': 'pour vos propres fichiers sur le réseau.',
    'Setting up': 'Installation en cours',
    'This takes a minute or two. Keep this page open.': 'Cela prend une minute ou deux. Gardez cette page ouverte.',
    'It asks for': 'Elle demande',
    'Now:': 'En cours :',
    'Setup stopped': 'L’installation s’est arrêtée',
    'It stopped at:': 'Arrêt à l’étape :',
    'What is already done is kept.': 'Ce qui est déjà fait est conservé.',
    '(the only one on this computer)': '(la seule sur cet ordinateur)',
    'This disk has': 'Ce disque a',
    'free.': 'de libre.',
    'Secret': 'Secret',
    'Checking the connection': 'Vérification de la connexion',
    'Starting in the background': 'Démarrage en arrière-plan',
    'Running and up to date': 'En marche et à jour',
    'Paused: your files wait until you resume': 'En pause : vos fichiers attendent que vous repreniez',
    'Running, but it has not synced for a while': 'En marche, mais pas synchronisé depuis un moment',
    'Running, but your files are not safe yet: add a machine to this account so copies exist elsewhere': 'En marche, mais vos fichiers ne sont pas encore en sécurité : ajoutez une machine à ce compte pour qu\'il en existe des copies ailleurs',
    'Running, but this disk can no longer hold what it promised to others': 'En marche, mais ce disque ne peut plus garder ce qu\'il a promis aux autres',
    'Not running': 'Arrêté',
    'This machine has left the network': 'Cette machine a quitté le réseau',
    'Cannot tell right now': 'Impossible à dire pour l’instant',
    'All set': 'C’est prêt',
    'Setup finished': 'Installation terminée',
    'ITSaNAS runs on this machine, and its icon is near the clock. A welcome.txt file is in your folder: open it on your other machines to see that they sync.':
      'ITSaNAS tourne sur cette machine, et son icône est près de l’horloge. Un fichier welcome.txt est dans votre dossier : ouvrez-le sur vos autres machines pour voir qu’elles se synchronisent.',
    'This computer already has the ITSaNAS account': 'Cet ordinateur a déjà le compte ITSaNAS',
    'Start continues with it. To set up a different account beside it, give it a name:': 'Commencer continue avec lui. Pour installer un autre compte à côté, donnez-lui un nom :',
    'Name of the other setup (lowercase letters and digits)': 'Nom de l’autre installation (minuscules et chiffres)',
    'Set up another account': 'Installer un autre compte',
    'The setup of the other account opened in a new page.': 'L’installation de l’autre compte s’est ouverte dans une nouvelle page.',
    'To remove the account already here instead, use Remove this machine in its icon near the clock, then run this setup again.': 'Pour supprimer plutôt le compte déjà présent, utilisez « Remove this machine » dans son icône près de l’horloge, puis relancez cette installation.',
    'Already done on this machine.': 'Déjà fait sur cette machine.',
    'This machine is already set up.': 'Cette machine est déjà installée.',
    'This account is already on this machine; its keys were not touched.': 'Ce compte est déjà sur cette machine ; ses clés n’ont pas été touchées.',
    'The folder is already chosen and kept in step.': 'Le dossier est déjà choisi et synchronisé.',
    'The space offered is already set; change it later in Settings.': 'L’espace offert est déjà réglé ; vous pourrez le changer dans les Réglages.',
    'Updates are already set.': 'Les mises à jour sont déjà réglées.',
    'This machine is already known to the coordinator.': 'Cette machine est déjà connue du coordinateur.',
    'Every step is done. The checks below could not run here; each says why.':
      'Toutes les étapes sont faites. Les vérifications ci-dessous n’ont pas pu tourner ici ; chacune dit pourquoi.',
    'Choose a username.': 'Choisissez un nom d’utilisateur.',
    'Type a whole number of GB, at least 1.': 'Tapez un nombre entier de Go, au moins 1.',
    'Three questions, then ITSaNAS sets itself up. If something stops, run setup again and it picks up where it left off.':
      'Trois questions, puis ITSaNAS s’installe tout seul. Si quelque chose s’arrête, relancez l’installation : elle reprend là où elle en était.',
    'Your recovery words and your passphrase are asked later in a separate ITSaNAS window, never in this page.':
      'Vos mots de récupération et votre phrase de passe seront demandés plus tard dans une fenêtre ITSaNAS séparée, jamais dans cette page.',
    'Your folder and your space': 'Votre dossier et votre espace',
    'How much of this disk other members may use for their copies: it is what earns room for your own files on theirs.':
      'Combien de ce disque les autres membres peuvent utiliser pour leurs copies : c’est ce qui vous donne de la place pour vos fichiers chez eux.',
    'Folder and space': 'Dossier et espace',
    'Disks':
      'Disques',
    'These are done by ITSaNAS running in the background, so it must be running. Nothing here asks for your passphrase.':
      'C’est ITSaNAS en arrière-plan qui s’en charge : il doit être lancé. Rien ici ne demande votre phrase secrète.',
    'Space for others is kept in:':
      'L’espace pour les autres est gardé dans :',
    'Move it to another disk':
      'Le déplacer sur un autre disque',
    'Move':
      'Déplacer',
    'That disk also holds the ITSaNAS data of this machine (less safe)':
      'Ce disque contient aussi les données ITSaNAS de cette machine (moins sûr)',
    'It is copied, checked, then used. The old copy stays until you delete it.':
      'Il est copié, vérifié, puis utilisé. L’ancienne copie reste jusqu’à ce que vous la supprimiez.',
    'Backup to a drive you keep apart':
      'Sauvegarde sur un disque gardé à part',
    'Back up':
      'Sauvegarder',
    'Restore from it':
      'Restaurer depuis celle-ci',
    'Sealed: without your 24 words it opens for nobody. On a new machine, set it up with your account first, then restore here.':
      'Scellée : sans vos 24 mots, elle ne s’ouvre pour personne. Sur une nouvelle machine, installez-la d’abord avec votre compte, puis restaurez ici.',
    'Put a portable ITSaNAS on a drive':
      'Mettre un ITSaNAS portable sur un disque',
    'Write it':
      'L’écrire',
    'It runs on this kind of system only, syncs your files and hosts nothing for others. Its keys travel with the drive, protected by your passphrase alone.':
      'Il ne tourne que sur ce type de système, synchronise vos fichiers et n’héberge ceux de personne. Ses clés voyagent avec le disque, protégées par votre seule phrase secrète.',
    'the default place, next to the data of this machine':
      'l’emplacement par défaut, à côté des données de cette machine',
    'Failed:':
      'Échec :',
    'Choose a folder first.':
      'Choisissez d’abord un dossier.',
    'Move the space for others there? ITSaNAS stops hosting for the time of the copy.':
      'Déplacer l’espace pour les autres ici ? ITSaNAS cesse d’héberger le temps de la copie.',
    'Read this backup into this machine?':
      'Lire cette sauvegarde dans cette machine ?',
    'Write a portable ITSaNAS there? Whoever has the drive can try to guess your passphrase: use a long one.':
      'Écrire un ITSaNAS portable ici ? Qui a le disque peut essayer de deviner votre phrase secrète : prenez-en une longue.',

    // Settings, set by the script (hand test, 2026-10-22: these showed in English).
    'ITSaNAS stopped serving this page. Run the command again to reopen it.':
      'ITSaNAS ne sert plus cette page. Relancez la commande pour la rouvrir.',
    'Refused':
      'Refusé',
    "This page's key is missing. Open the address that the command printed in your terminal.":
      'La clé de cette page manque. Ouvrez l’adresse que la commande a affichée dans votre terminal.',
    'passed':
      'réussi',
    'failed':
      'échoué',
    'skipped':
      'non vérifié',
    'every':
      'toutes les',
    '(now)':
      '(actuel)',
    'Type a whole number of GB.':
      'Tapez un nombre entier de Go.',
    'Nothing was changed.':
      'Rien n’a été changé.',
    'ITSaNAS restarts (a few seconds).':
      'ITSaNAS redémarre (quelques secondes).',
    'Saved. ITSaNAS runs again.':
      'Enregistré. ITSaNAS tourne de nouveau.',
    'Saved.':
      'Enregistré.',
    'Not saved:':
      'Non enregistré :',
    'Sign out of ITSaNAS on this machine?':
      'Se déconnecter d’ITSaNAS sur cette machine ?',
    "Syncing stops, and this machine stops answering the other members' checks until you sign in again. Your files and this machine's keys stay on this disk.":
      'La synchronisation s’arrête, et cette machine ne répond plus aux vérifications des autres membres jusqu’à ce que vous vous reconnectiez. Vos fichiers et les clés de cette machine restent sur ce disque.',

    // What the server says to Settings, line by line (said() in app.js).
    'Syncing paused on this machine until you resume. Nothing is lost: your changes here wait until syncing resumes, and this machine keeps hosting for the others. A transfer under way finishes with the machine it is talking to (five minutes at most); no other starts.':
      'Synchronisation en pause sur cette machine jusqu’à ce que vous repreniez. Rien n’est perdu : vos modifications ici attendent la reprise, et cette machine continue d’héberger pour les autres. Un transfert en cours se termine avec la machine concernée (cinq minutes au plus) ; aucun autre ne commence.',
    'Resume sooner with: itsanas resume':
      'Pour reprendre plus tôt : itsanas resume',
    '(No daemon is running this node now; it reads this when it starts.)':
      '(ITSaNAS ne tourne pas sur cette machine pour l’instant ; il le lira à son démarrage.)',
    'Syncing was already paused.':
      'La synchronisation était déjà en pause.',
    'Syncing was not paused.':
      'La synchronisation n’était pas en pause.',
    'Syncing resumed; a round starts within two seconds.':
      'Synchronisation reprise ; un tour commence d’ici deux secondes.',
    'syncing is paused on this machine; `itsanas resume` resumes and syncs at once':
      'la synchronisation est en pause sur cette machine ; Reprendre la relance et synchronise aussitôt',
    'no daemon is running this node; start it, and it syncs as it starts':
      'ITSaNAS ne tourne pas sur cette machine ; démarrez-le, il synchronise dès son démarrage',
    'Asked; the round starts within two seconds.':
      'Demandé ; le tour commence d’ici deux secondes.',
    'The daemon syncs on its own setting again.':
      'ITSaNAS synchronise de nouveau selon son propre réglage.',
    'the ITSaNAS service is not running on this machine: start it (Start ITSaNAS, or `itsanas daemon`), or do this in a terminal with `itsanas vault --move` or `itsanas backup`':
      'le service ITSaNAS ne tourne pas sur cette machine : démarrez-le (Start ITSaNAS, ou `itsanas daemon`), ou faites-le dans un terminal avec `itsanas vault --move` ou `itsanas backup`',
    'asked; the service starts within two seconds':
      'demandé ; le service commence d’ici deux secondes',

    // "Copy this account to a USB key" (#st-usb).
    'Copy this account to a USB key':
      'Copier ce compte sur une clé USB',
    'The key gets ITSaNAS for Windows, Mac and Linux, with this account in it: plug it into any computer, open it, and your files are there.':
      'La clé reçoit ITSaNAS pour Windows, Mac et Linux, avec ce compte dedans : branchez-la sur n’importe quel ordinateur, ouvrez-la, et vos fichiers sont là.',
    'Whoever opens the key with its passphrase IS this account: they can read and change all your files. Choose a long passphrase that you use nowhere else, and keep the key as safe as a house key.':
      'Quiconque ouvre la clé avec sa phrase de passe EST ce compte : il peut lire et modifier tous vos fichiers. Choisissez une longue phrase de passe que vous n’utilisez nulle part ailleurs, et gardez la clé aussi précieusement qu’une clé de maison.',
    'USB keys and external disks':
      'Clés USB et disques externes',
    'No USB key or external disk was found. Plug one in, then Refresh.':
      'Aucune clé USB ni aucun disque externe trouvé. Branchez-en un, puis Actualiser.',
    'Another folder':
      'Autre dossier',
    'Another folder…':
      'Autre dossier…',
    'Copy here':
      'Copier ici',
    'Refresh':
      'Actualiser',
    'Two ITSaNAS windows open, one after the other:':
      'Deux fenêtres ITSaNAS s’ouvrent, l’une après l’autre :',
    "first this machine's passphrase, then a new passphrase for the key, typed twice. If you cannot see them, look for ITSaNAS in your taskbar / Dock.":
      'd’abord la phrase de passe de cette machine, puis une nouvelle phrase de passe pour la clé, tapée deux fois. Si vous ne les voyez pas, cherchez ITSaNAS dans votre barre des tâches / le Dock.',
    'Too little room on it for the copy.':
      'Trop peu de place dessus pour la copie.',
    'This key already holds an ITSaNAS account: choose another key, or empty this one first.':
      'Cette clé contient déjà un compte ITSaNAS : choisissez une autre clé, ou videz celle-ci d’abord.',
    "This is where this account already lives: choose the key's own drive.":
      'C’est là que ce compte se trouve déjà : choisissez le lecteur de la clé elle-même.',
    'Nothing is there: is the key plugged in?':
      'Il n’y a rien ici : la clé est-elle branchée ?',
    'Type a full path, starting with / or a drive letter.':
      'Tapez un chemin complet, qui commence par / ou une lettre de lecteur.',
    'NTFS: a Mac can read this key but not write to it, so the copy will not start on a Mac. Reformat it as exFAT to use it everywhere.':
      'NTFS : un Mac peut lire cette clé mais pas y écrire, donc la copie ne démarrera pas sur un Mac. Reformatez-la en exFAT pour l’utiliser partout.',
    'FAT32: no file on it may exceed 4 GB, and your account can grow past that. exFAT has no such limit.':
      'FAT32 : aucun fichier n’y peut dépasser 4 Go, et votre compte peut grandir au-delà. exFAT n’a pas cette limite.',
    'Copying in the background':
      'Copie en arrière-plan',
    'Checking the key':
      'Vérification de la clé',
    "Opening this machine's keys":
      'Ouverture des clés de cette machine',
    'Checking the room on the key':
      'Vérification de la place sur la clé',
    'Writing the programs for every system on the key':
      'Écriture des programmes pour chaque système sur la clé',
    'Sealing the account on the key under its own passphrase':
      'Scellement du compte sur la clé sous sa propre phrase de passe',
    'Enrolling the key with the coordinator':
      'Inscription de la clé auprès du coordinateur',
    'free,':
      'de libre,',
    'about':
      'environ',
    'needed.':
      'nécessaires.',
    'free of':
      'de libre sur',
    'external disk?':
      'disque externe ?',
    'The copy needs about':
      'La copie demande environ',
    'on the key:':
      'sur la clé :',
    'for the programs':
      'pour les programmes',
    'for your files.':
      'pour vos fichiers.',
    'The size of your files is not known yet.':
      'La taille de vos fichiers n’est pas encore connue.',
    'Waiting for the passphrase windows…':
      'En attente des fenêtres de phrase de passe…',
    'The key is ready. Unplug it safely before taking it out.':
      'La clé est prête. Éjectez-la proprement avant de la retirer.',
    'The copy stopped:':
      'La copie s’est arrêtée :',
    'The copy stopped without saying why. Try again; a key it left half written is refused and must be emptied first.':
      'La copie s’est arrêtée sans dire pourquoi. Réessayez ; une clé qu’elle a laissée à moitié écrite est refusée et doit d’abord être vidée.',
  },
};
