  // ------------------------------------------------------------ trash and rules
  //
  // The rules (/api/deletion) and the trash (/api/trash) go through the node.
  // What keeps less than now is a job: the engine asks who is there in a window
  // of the system's own, once for the whole selection, and this page follows it
  // through /api/state as it does the other windows.
  let deletionRules = null;

  function ruleForm() {
    const checked = function (name) {
      const box = document.querySelector('input[name="' + name + '"]:checked');
      return box ? box.value : '';
    };
    return {
      mode: checked('st-del-mode'),
      receive: checked('st-del-receive'),
      days: $('st-del-days').value,
      ask_auth: $('st-del-auth').checked ? 'true' : 'false',
      confirm: $('st-del-confirm').checked ? 'true' : 'false',
    };
  }

  function ruleWords(rules) {
    const mode = { trash: t('to the trash'), direct: t('at once, everywhere'), local: t('from this machine only') }[rules.mode] || rules.mode;
    const receive = rules.receive === 'protect' ? t('keeps what others delete') : t('lets it go');
    return mode + ' \u00b7 ' + receive + ' \u00b7 ' + rules.days + ' ' + t('days');
  }

  function showRules(reply) {
    deletionRules = reply;
    const here = reply.here;
    document.querySelectorAll('input[name="st-del-mode"]').forEach(function (box) { box.checked = box.value === here.mode; });
    document.querySelectorAll('input[name="st-del-receive"]').forEach(function (box) { box.checked = box.value === here.receive; });
    $('st-del-days').value = here.days;
    $('st-del-auth').checked = here.ask_auth;
    $('st-del-confirm').checked = here.confirm;
    $('st-del-follow').hidden = !reply.own;
    $('st-del-account').textContent = t('The account:') + ' ' + ruleWords(reply.account) +
      (reply.own ? '' : ' \u00b7 ' + t('this machine follows it'));
    const list = $('st-del-machines');
    list.textContent = '';
    reply.machines.forEach(function (machine) {
      const item = document.createElement('li');
      item.textContent = machine.device.slice(0, 12) + (machine.me ? ' \u00b7 ' + t('this machine') : '') + ': ' + ruleWords(machine.rules);
      list.appendChild(item);
    });
  }

  async function loadRules() {
    try { showRules(await api('GET', '/api/deletion')); } catch (e) { $('st-del-said').textContent = e.message; }
  }

  // A job the engine started: follow it until it is over, then say how it ended.
  async function followJob(line) {
    for (;;) {
      await new Promise(function (resolve) { setTimeout(resolve, 1000); });
      const state = await refreshSettings();
      if (!state || state.run.phase === 'running') { continue; }
      if (state.run.phase === 'done') {
        line.textContent = said(state.run.lines[state.run.lines.length - 1] || '');
      } else {
        const failed = state.run.failed || { error: '', remedy: '' };
        line.textContent = t('Not changed:') + ' ' + said(failed.error);
      }
      return;
    }
  }

  // How long the traces of deletes are kept (/api/traces): one choice for the
  // account, on a slider over the durations the engine offers.
  let traceChoices = [];
  function traceWords() {
    return [t('10 days'), t('15 days'), t('20 days'), t('30 days'), t('45 days'),
      t('2 months'), t('3 months'), t('6 months'), t('1 year'), t('18 months'),
      t('2 years'), t('3 years'), t('5 years')];
  }
  function showTraceValue() {
    const never = $('st-traces-never').checked;
    $('st-traces-slider').disabled = never;
    $('st-traces-value').textContent = never ? '' : (traceWords()[Number($('st-traces-slider').value)] || '');
  }
  function showTraces(reply) {
    traceChoices = reply.choices || [];
    $('st-traces-slider').max = String(Math.max(traceChoices.length - 1, 0));
    const days = reply.forever ? reply.default : reply.days;
    const at = traceChoices.indexOf(days);
    $('st-traces-slider').value = String(at < 0 ? traceChoices.indexOf(reply.default) : at);
    $('st-traces-never').checked = !!reply.forever;
    showTraceValue();
  }
  async function loadTraces() {
    try { showTraces(await api('GET', '/api/traces')); } catch (e) { $('st-traces-said').textContent = e.message; }
  }
  async function saveTraces() {
    const line = $('st-traces-said');
    line.textContent = '';
    const keep = $('st-traces-never').checked ? 'never' : String(traceChoices[Number($('st-traces-slider').value)]);
    let reply;
    try { reply = await api('POST', '/api/traces', { keep: keep }); } catch (e) { line.textContent = e.message; return; }
    if (reply.started) {
      line.textContent = t('A window asks who you are.');
      await followJob(line);
      await loadTraces();
      return;
    }
    showTraces(reply);
    line.textContent = t('Saved.');
  }

  // What a drive is for decides which of three tools it wants (HANDOVER §8
  // 0e step 7): the one asked points at the field, and the other two stay.
  function diskUse(choice) {
    const said = $('disk-use-said');
    const go = {
      drawer: ['A backup: sealed, it opens with your 24 words only. Use “Backup to a drive you keep apart” below.', 'disk-backup'],
      mine: ['A USB key: ITSaNAS for every system, with this account on it. Use “Copy this account to a USB key”.', 'st-h-usb'],
      others: ['A portable ITSaNAS: it hosts nothing for others, and its keys stay on the drive. Use “Put a portable ITSaNAS on a drive” below.', 'disk-portable'],
    }[choice];
    if (!go) { said.textContent = ''; return; }
    said.textContent = t(go[0]);
    const target = $(go[1]);
    if (target && !target.closest('[hidden]')) { target.scrollIntoView({ block: 'center' }); target.focus(); }
  }

  async function saveRules(scope) {
    const line = $('st-del-said');
    line.textContent = '';
    const form = ruleForm();
    form.scope = scope;
    if (scope === 'all' && !window.confirm(t('Every machine of the account follows these rules, and loses its own. Go on?'))) { return; }
    if (scope !== 'all' && deletionRules && form.mode !== deletionRules.here.mode &&
        !window.confirm(t('Change what a delete does on this machine?'))) { return; }
    let reply;
    try { reply = await api('POST', '/api/deletion', form); } catch (e) { line.textContent = e.message; return; }
    if (reply.started) {
      line.textContent = t('A window asks who you are.');
      await followJob(line);
      await loadRules();
      return;
    }
    showRules(reply);
    line.textContent = t('Saved.');
  }

  function chosenPaths() {
    return Array.prototype.slice.call(document.querySelectorAll('#st-trash input[type="checkbox"]:checked'))
      .map(function (box) { return box.value; });
  }

  async function trash(action, paths) {
    const line = $('st-trash-said');
    const list = $('st-trash');
    line.textContent = '';
    const confirmFirst = !deletionRules || deletionRules.here.confirm;
    if (action === 'delete' && confirmFirst &&
        !window.confirm(t('It disappears from every machine of the account. There is no undo.') + '\n' + paths.join('\n'))) { return; }
    if (action === 'empty' && confirmFirst &&
        !window.confirm(t('Every file in the trash disappears from every machine of the account. There is no undo.'))) { return; }
    let reply;
    try {
      reply = action
        ? await api('POST', '/api/trash', { action: action, paths: (paths || []).join('\n') })
        : await api('GET', '/api/trash');
    } catch (e) { line.textContent = e.message; return; }
    if (reply.started) {
      line.textContent = t('A window asks who you are.');
      await followJob(line);
      return trash(null, []);
    }
    list.textContent = '';
    $('st-trash-delete').hidden = !reply.trashed.length;
    $('st-trash-empty').hidden = !reply.trashed.length;
    if (!reply.trashed.length) {
      const empty = document.createElement('li');
      empty.textContent = t('The trash is empty.');
      list.appendChild(empty);
    }
    const kinds = {
      shared: t('in the account\'s trash'),
      here: t('in this machine\'s trash'),
      waiting: t('waits for you to confirm'),
      protected: t('kept here after another machine deleted it'),
    };
    reply.trashed.forEach(function (row) {
      const item = document.createElement('li');
      const label = document.createElement('label');
      label.className = 'choice';
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.value = row.path;
      label.appendChild(box);
      const days = Math.max(0, Math.ceil((row.due - reply.now) / 86400));
      label.appendChild(document.createTextNode(' ' + row.path + ' \u00b7 ' + (kinds[row.kind] || row.kind) +
        (row.kind === 'waiting' ? '' : ' \u00b7 ' + t('days before it is really deleted:') + ' ' + days)));
      item.appendChild(label);
      const restore = document.createElement('button');
      restore.type = 'button';
      restore.textContent = t('Restore');
      restore.addEventListener('click', function () { trash('restore', [row.path]); });
      item.appendChild(restore);
      list.appendChild(item);
    });
    line.textContent = said(reply.said || '');
  }

  // ------------------------------------------------- files on this machine
  // The versions of one file, newest first; any but the current one comes back
  // on every machine, fetched first when this machine never held it.
  function versionList(path, versions) {
    const list = document.createElement('ol');
    versions.forEach(function (version) {
      const item = document.createElement('li');
      const when = new Date(version.modified * 1000).toLocaleString();
      item.textContent = when + ' · ' + version.size + ' B' +
        (version.current ? ' · ' + t('current') : version.here ? '' : ' · ' + t('not here'));
      if (!version.current) {
        const back = document.createElement('button');
        back.type = 'button';
        back.textContent = t('Bring back');
        back.addEventListener('click', function () { files('restore', path, { id: version.id }); });
        item.appendChild(back);
      }
      list.appendChild(item);
    });
    return list;
  }

  async function files(action, path, more) {
    const line = $('st-files-said');
    const list = $('st-files');
    line.textContent = '';
    let reply;
    try {
      reply = action ? await api('POST', '/api/files', Object.assign({ action: action, path: path }, more || {})) : await api('GET', '/api/files');
    } catch (e) { line.textContent = e.message; return; }
    list.textContent = '';
    reply.files.forEach(function (row) {
      const item = document.createElement('li');
      item.dataset.path = row.path.toLowerCase();
      const label = document.createElement('span');
      label.textContent = row.path + ' \u00b7 ' + row.size + ' \u00b7 ' + (row.freed ? t('freed here') : row.here ? t('here') : t('not here'));
      item.appendChild(label);
      if (reply.folder) {
        const button = document.createElement('button');
        button.type = 'button';
        button.textContent = row.freed ? t('Keep here') : t('Free up');
        button.addEventListener('click', function () { files(row.freed ? 'keep' : 'free', row.path); });
        item.appendChild(button);
      }
      // Every version of the file, and a new name, on every machine (§8 15).
      const history = document.createElement('button');
      history.type = 'button';
      history.textContent = t('Versions');
      history.addEventListener('click', function () { files('versions', row.path); });
      item.appendChild(history);
      const rename = document.createElement('button');
      rename.type = 'button';
      rename.textContent = t('Rename');
      rename.addEventListener('click', function () {
        const to = window.prompt(t('New name, on every machine:'), row.path);
        if (to && to !== row.path) { files('rename', row.path, { to: to }); }
      });
      item.appendChild(rename);
      if (reply.versions && reply.versions.path === row.path) {
        item.appendChild(versionList(row.path, reply.versions.list));
      }
      // Not on this machine: delete it from every machine, into the account's trash.
      if (!row.here) {
        const away = document.createElement('button');
        away.type = 'button';
        away.textContent = t('Delete everywhere');
        away.addEventListener('click', function () {
          const ask = !deletionRules || deletionRules.here.confirm;
          if (ask && !window.confirm(t('It goes to the account\'s trash and leaves every machine when its days are over.') + '\n' + row.path)) { return; }
          files('everywhere', row.path);
        });
        item.appendChild(away);
      }
      list.appendChild(item);
    });
    if (reply.said) {
      line.textContent = said(reply.said);
    } else if (!reply.complete) {
      line.textContent = t('Not every file is listed: `itsanas ls` shows them all.');
    }
  }

