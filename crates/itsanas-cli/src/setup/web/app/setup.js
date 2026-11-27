  // ---------------------------------------------------------------- setup

  // The page's screens, and the engine steps each one holds (setup/mod.rs
  // STEPS). Three questions, then the run: every screen is one more place an
  // impatient person leaves (Nicolas, 2026-10-07).
  const STEPS = [
    { page: 'welcome', label: 'Welcome', engines: ['machine'] },
    { page: 'account', label: 'Account', engines: ['account', 'secret'] },
    { page: 'folder', label: 'Folder and space', engines: ['registration', 'folder', 'pledge', 'updates'] },
    { page: 'connection', label: 'Setting up', engines: ['connectivity', 'service'] },
    { page: 'final', label: 'Final check', engines: ['verify'] },
  ];
  const QUESTION_PAGES = ['welcome', 'account', 'folder'];
  function stepDone(step) { return step.engines.every(function (e) { return done[e]; }); }
  function markEngines(step) { step.engines.forEach(function (e) { done[e] = true; }); }
  const done = {};
  let current = 'welcome';
  let polling = null;

  function buildProgress() {
    const list = $('progress');
    list.textContent = '';
    STEPS.forEach(function (step, index) {
      const item = document.createElement('li');
      item.textContent = t(step.label);
      item.dataset.n = String(index + 1);
      item.dataset.page = step.page;
      list.appendChild(item);
    });
  }

  function markProgress(currentPage, failedPage) {
    document.querySelectorAll('#progress li').forEach(function (item) {
      const step = STEPS.find(function (s) { return s.page === item.dataset.page; });
      const isCurrent = item.dataset.page === currentPage;
      if (isCurrent) { item.setAttribute('aria-current', 'step'); }
      else { item.removeAttribute('aria-current'); }
      item.classList.toggle('done', !isCurrent && stepDone(step));
      item.classList.toggle('failed', item.dataset.page === failedPage);
    });
  }

  function go(page) {
    current = page;
    showPage(page);
    markProgress(page);
  }

  async function step(delta) {
    const index = QUESTION_PAGES.indexOf(current);
    if (delta > 0 && !(await validate(current))) { return; }
    const next = QUESTION_PAGES[index + delta];
    if (next) { go(next); }
  }

  // Null until one is chosen: a preselected answer made a new account for a
  // person who meant to join their own (Nicolas, 2026-10-14).
  function accountKind() {
    const chosen = document.querySelector('input[name="account"]:checked');
    return chosen ? chosen.value : null;
  }

  async function validate(page) {
    if (page === 'account' && !done.account && !accountKind()) {
      say('Say whether you already have an account: choose one of the two answers.');
      document.querySelector('input[name="account"]').focus();
      return false;
    }
    if (page === 'account' && !done.account && !$('username').value.trim()) {
      say('Choose a username.');
      $('username').focus();
      return false;
    }
    if (page === 'account' && !done.account) {
      // A fresh answer, never the last one the timer fetched: a quick Next
      // would otherwise go on with the answer to a name since changed.
      await checkName();
      const asked = Boolean($('coordinator').value.trim());
      if (nameInvalid) {
        say(nameInvalid);
        $('username').focus();
        return false;
      }
      if (accountKind() === 'new' && nameTaken === 'yes') {
        say('This username is already taken. Choose another.');
        $('username').focus();
        return false;
      }
      // No next step without a positive check: a network that cannot be asked
      // has not said the name is free. With no coordinator there is nobody to ask.
      if (accountKind() === 'new' && asked && nameTaken !== 'no') {
        say('The network cannot be asked right now. A new account is made only once the network says this username is free.');
        $('username-retry').hidden = false;
        return false;
      }
      if (accountKind() === 'join' && asked && nameTaken === 'no' && !$('continue-name').checked) {
        say('No account has this name on the network. Tick "Continue with this name" if you are sure it is the name of your account, or correct it.');
        $('continue-name').focus();
        return false;
      }
    }
    if ((page === 'folder') && $('pledge').value !== '' && !/^[1-9]\d*$/.test($('pledge').value)) {
      say('Type a whole number of GB, at least 1.');
      $('pledge').focus();
      return false;
    }
    return true;
  }

  // ------------------------------------------------- username, while typed

  let nameTaken = 'unknown';
  let nameInvalid = '';
  let nameTimer = null;

  async function checkName() {
    const username = $('username').value.trim();
    const coordinator = $('coordinator').value.trim();
    const line = $('username-check');
    nameTaken = 'unknown';
    nameInvalid = '';
    line.textContent = '';
    $('username-retry').hidden = true;
    $('continue-name-row').hidden = true;
    if (!username || done.account) { return; }
    try {
      const reply = await api('POST', '/api/name', { username: username, coordinator: coordinator, account: accountKind() || '' });
      if (username !== $('username').value.trim()) { return; }
      nameTaken = reply.taken;
      nameInvalid = reply.invalid || '';
      const isNew = accountKind() === 'new';
      if (nameInvalid) {
        line.textContent = t(nameInvalid);
      } else if (!accountKind() || !coordinator) {
        // What the answer means depends on the question not answered yet, and
        // answering it asks again.
        line.textContent = '';
      } else if (reply.taken === 'yes') {
        line.textContent = isNew ? t('This username is already taken. Choose another.') : t('Account found.');
        if (!isNew) { suggestSpace(reply.other_pledge_bytes); }
      } else if (reply.taken === 'no') {
        line.textContent = isNew
          ? t('This username is free.')
          : t('No account has this name on the network. A name is bound to an account when it first joins: be sure it is the one you used on your other machines.');
        $('continue-name-row').hidden = isNew;
      } else {
        line.textContent = t(isNew
          ? 'The network cannot be asked right now. A new account is made only once the network says this username is free.'
          : 'The network cannot be asked right now; it is checked again before anything is made.');
        $('username-retry').hidden = !isNew;
      }
    } catch (e) { /* the page asks again before it goes on */ }
  }

  function nameChanged() {
    // The coordinator takes lowercase names only: say it by typing it.
    const box = $('username');
    const low = box.value.toLowerCase();
    if (low !== box.value) { box.value = low; }
    $('continue-name').checked = false;
    if (nameTimer) { clearTimeout(nameTimer); }
    nameTimer = setTimeout(checkName, 500);
  }

  // ------------------------------------------------------- space and folder

  const GIB = 1073741824;
  let split = { own: 30, network: 70 };

  function showEarns() {
    const offered = parseInt($('pledge').value, 10);
    $('pledge-earns').textContent = offered > 0
      ? t('You offer') + ' ' + offered + ' GB → ' + t('you get about') + ' ' +
        Math.floor(offered * split.own / split.network) + ' GB ' + t('for your own files on the network.')
      : '';
  }

  function setupSpace(defaults) {
    split = { own: defaults.split_own, network: defaults.split_network };
    const max = Math.max(1, Math.floor(Number(defaults.free_bytes) / GIB));
    const slider = $('pledge-slider');
    slider.max = String(max);
    $('pledge').max = String(max);
    const start = Math.min(max, Math.max(1, defaults.pledge_gb));
    slider.value = String(start);
    $('pledge').value = String(start);
    $('keep').value = String(defaults.keep_gb);
    slider.addEventListener('input', function () { $('pledge').value = slider.value; showEarns(); });
    $('pledge').addEventListener('input', function () {
      if (/^\d+$/.test($('pledge').value)) { slider.value = $('pledge').value; }
      showEarns();
    });
    showEarns();
  }

  // What to offer and keep, from the one rule every platform shares (the
  // server's): asked again once the person says which account they join, since
  // "the same as your other machines" is only known then.
  async function suggestSpace(other) {
    try {
      const reply = await api('POST', '/api/suggest', { other: String(other || 0) });
      const max = parseInt($('pledge-slider').max, 10);
      const gb = Math.min(max, Math.max(1, Number(reply.pledge_gb)));
      $('pledge-slider').value = String(gb);
      $('pledge').value = String(gb);
      $('keep').value = String(reply.keep_gb);
      const line = $('space-same');
      if (reply.same_as_other_machines) {
        line.textContent = t('The same as your other machines.');
      } else if (other > 0) {
        line.textContent = t('Your other machines offer') + ' ' + Math.round(other / GIB) + ' GB. ' + t('This one has room for less.');
      } else {
        line.textContent = '';
      }
      showEarns();
    } catch (e) { /* the suggestion made at load stays */ }
  }

  async function browse(input) {
    try {
      const reply = await api('POST', '/api/pick', { start: input.value.trim() });
      if (reply.path) { input.value = reply.path; }
      return reply;
    } catch (e) { say(e.message); return {}; }
  }

  // Other people's copies on another disk: the space offered is that disk's.
  async function browseVault() {
    const reply = await browse($('vault'));
    if (!reply.path || reply.free_bytes === null || reply.free_bytes === undefined) { return; }
    const max = Math.max(1, Math.floor(Number(reply.free_bytes) / GIB));
    $('pledge-slider').max = String(max);
    $('pledge').max = String(max);
    if (parseInt($('pledge').value, 10) > max) { $('pledge').value = String(max); }
    $('pledge-slider').value = $('pledge').value;
    $('space-free').textContent = t('Free on that disk:') + ' ' + max + ' GB';
    showEarns();
  }

  async function folderAction(path, pin, line) {
    try {
      const reply = await api('POST', pin ? '/api/pin' : '/api/open', path ? { path: path } : {});
      if (line) { line.textContent = said(reply.said); }
    } catch (e) { if (line) { line.textContent = e.message; } else { say(e.message); } }
  }

  const DONE_SAID = {
    machine: N('This machine is already set up.'),
    account: N('This account is already on this machine; its keys were not touched.'),
    folder: N('The folder is already chosen and kept in step.'),
    pledge: N('The space offered is already set; change it later in Settings.'),
    updates: N('Updates are already set.'),
    registration: N('This machine is already known to the coordinator.'),
  };

  function showDone(engine, said) {
    done[engine] = true;
    const note = document.querySelector('[data-done-for="' + engine + '"]');
    if (note) {
      // A sentence in the page's language; the engine's own words (paths,
      // names, in English) stay on hover rather than in the middle of it.
      show(note, DONE_SAID[engine] || N('Already done on this machine.'));
      note.title = said.replace(/^done:\s*/, '');
      note.hidden = false;
    }
  }

  async function loadPlan() {
    try {
      const plan = await api('GET', '/api/plan');
      plan.forEach(function (row) { if (row.done) { showDone(row.step, row.said); } });
    } catch (e) {
      // Not knowing which steps are done only means asking again; the
      // engine skips what is done whatever the page sends.
    }
    $('account-questions').hidden = Boolean(done.account);
    // A done step shows no field to fill: it looked as if it had to be
    // answered again, with a default that was not the value in force.
    if (done.pledge) { $('pledge').value = ''; $('pledge-questions').hidden = true; }
    if (done.folder) { $('folder').value = ''; $('folder-questions').hidden = true; }
  }

  function setupForm() {
    const form = {};
    if (!done.account) {
      form.account = accountKind();
      form.username = $('username').value.trim();
      if (form.account === 'join' && $('recover-escrow').checked && $('coordinator').value.trim()) {
        form.recover_from = $('coordinator').value.trim();
      }
    }
    ['coordinator', 'invite', 'folder', 'vault'].forEach(function (id) {
      const value = $(id).value.trim();
      if (value) { form[id] = value; }
    });
    if ($('pledge').value !== '') { form.pledge = $('pledge').value + 'G'; }
    if (!$('keep-questions').hasAttribute('data-off')) {
      form.keep = $('keep').value === '' ? '' : $('keep').value + 'G';
    }
    const updates = document.querySelector('input[name="updates"]:checked');
    if (updates) { form.updates = updates.value; }
    form.background = $('background').checked ? 'yes' : 'no';
    return form;
  }

  async function startSetup() {
    if (current === 'folder' && !(await validate('folder'))) { return; }
    go('connection');
    $('start').disabled = true;
    try {
      await api('POST', '/api/run', setupForm());
      following();
    } catch (e) {
      say(e.message);
      $('start').disabled = false;
      $('start-actions').hidden = false;
    }
  }

  function following() {
    $('start-actions').hidden = true;
    $('run-panel').hidden = false;
    $('run-title').textContent = t('Setting up');
    $('run-lead').textContent = t('This takes a minute or two. Keep this page open.');
    document.querySelectorAll('[data-secret-warning]').forEach(function (w) { w.hidden = false; });
    if (!polling) { polling = setInterval(pollRun, 1000); }
    pollRun();
  }

  function pageOfEngine(engine) {
    const found = STEPS.find(function (s) { return s.engines.indexOf(engine) >= 0; });
    return found ? found.page : 'welcome';
  }

  function renderLines(list, lines) {
    list.textContent = '';
    lines.forEach(function (line) {
      const item = document.createElement('li');
      item.textContent = line;
      list.appendChild(item);
    });
  }

  function renderWindow(notice, whatLine, waiting) {
    notice.hidden = !waiting;
    if (whatLine) { whatLine.textContent = waiting ? t('It asks for') + ' ' + waiting + '.' : ''; }
  }

  async function pollRun() {
    let state;
    try { state = await api('GET', '/api/state'); } catch (e) { say(e.message); return; }
    const run = state.run;
    if (run.step) {
      const page = pageOfEngine(run.step);
      const reached = STEPS.findIndex(function (s) { return s.page === page; });
      STEPS.slice(0, reached).forEach(markEngines);
      current = page;
      markProgress(page);
      const label = STEPS.find(function (s) { return s.page === page; }).label;
      $('run-step').textContent = t('Now:') + ' ' + t(label);
    }
    renderWindow($('window-notice'), $('window-what'), run.waiting);
    renderLines($('run-lines'), run.lines);
    if (run.phase === 'done' || run.phase === 'failed') {
      clearInterval(polling);
      polling = null;
      showFinal(run);
    }
  }

  const VERDICTS = { passed: N('passed'), failed: N('failed'), skipped: N('skipped') };

  // The final check's names (verify.rs) in a person's words.
  const FINDINGS = {
    'the daemon is healthy': N('ITSaNAS is running'),
    'the background service': N('The background service'),
    'listed by the coordinator': N('The network knows this device'),
    'reachable from outside': N('Other machines can reach this one'),
    'the folder syncs': N('The folder syncs'),
  };
  // What verify.rs says when it skips a check, marked so that each gets a translation.
  const FINDING_DETAILS = [
    N('the app does this itself'),
    N('a phone picks its folder in the app'),
    N('a phone dials out and is never dialled back'),
    N('no background service was asked for'),
    N('no coordinator is set'),
    N('no coordinator is set to dial back'),
    N('the coordinator was not reached'),
    N('no folder is set'),
    N('not asked'),
  ];
  void FINDING_DETAILS;

  function renderReport(list, report) {
    list.textContent = '';
    const marks = { passed: '✓', failed: '✗', skipped: '–' };
    report.forEach(function (finding) {
      const item = document.createElement('li');
      item.className = finding.verdict;
      const mark = document.createElement('span');
      mark.className = 'mark';
      mark.textContent = marks[finding.verdict] || '';
      mark.setAttribute('role', 'img');
      mark.setAttribute('aria-label', t(VERDICTS[finding.verdict] || finding.verdict));
      item.appendChild(mark);
      item.appendChild(document.createTextNode(t(FINDINGS[finding.what] || finding.what)));
      // What passed needs no more words: addresses and counts are for `itsanas setup`.
      if (finding.verdict !== 'passed') {
        const detail = document.createElement('div');
        detail.className = 'detail';
        detail.textContent = finding.verdict === 'failed' && finding.remedy
          ? said(finding.detail) + ' ' + t('What to do:') + ' ' + said(finding.remedy)
          : said(finding.detail);
        item.appendChild(detail);
      }
      list.appendChild(item);
    });
  }

  function showFinal(run) {
    current = 'final';
    showPage('final');
    renderReport($('report'), run.report);
    if (run.phase === 'done') {
      STEPS.forEach(markEngines);
      markProgress('final');
      // "Works" only when a check passed: a run whose checks were all
      // skipped (no background service asked for) proved nothing.
      const proved = run.report.some(function (f) { return f.verdict === 'passed'; });
      $('final-title').textContent = t(proved ? 'All set' : 'Setup finished');
      $('final-lead').textContent = t(proved
        ? (onPhone
          ? 'ITSaNAS runs on this phone. Your files are in the app, which keeps them in sync with your other machines.'
          : 'ITSaNAS runs on this machine, and its icon is near the clock. A welcome.txt file is in your folder: open it on your other machines to see that they sync.')
        : 'Every step is done. The checks below could not run here; each says why.');
      $('final-fix').hidden = true;
      $('retry').hidden = true;
      $('folder-actions').hidden = false;
    } else {
      const failed = run.failed || { step: 'machine', title: '', error: '', remedy: '' };
      markProgress('final', pageOfEngine(failed.step));
      $('final-title').textContent = t('Setup stopped');
      $('final-lead').textContent = t('It stopped at:') + ' ' + failed.title + '. ' + t('What is already done is kept.');
      $('final-remedy').textContent = failed.remedy;
      $('final-error').textContent = failed.error;
      $('final-fix').hidden = false;
      $('retry').hidden = false;
      // The account is at its device limit: the way out is a device to withdraw.
      $('final-devices').hidden = !failed.atCap;
      if (failed.atCap) { showDevices($('final-devices-list'), $('final-devices-said')); }
    }
  }

  async function retry() {
    await loadPlan();
    $('start').disabled = false;
    $('start-actions').hidden = false;
    $('run-panel').hidden = true;
    $('run-title').textContent = t('Ready');
    go('connection');
  }

  async function initSetup(state) {
    buildProgress();
    $('machine-home').textContent = state.home;
    $('machine-instance').textContent = state.instance || t('(the only one on this computer)');
    $('folder').value = state.defaults.folder_now || state.defaults.folder;
    $('coordinator').value = state.defaults.coordinator;
    setupSpace(state.defaults);
    $('username').addEventListener('input', nameChanged);
    $('username-retry').addEventListener('click', checkName);
    $('coordinator').addEventListener('change', nameChanged);
    $('folder-browse').addEventListener('click', function () { browse($('folder')); });
    $('vault-browse').addEventListener('click', browseVault);
    $('open-folder').addEventListener('click', function () { folderAction('', false, $('folder-said')); });
    $('pin-folder').addEventListener('click', function () { folderAction('', true, $('folder-said')); });
    const updates = document.querySelector('input[name="updates"][value="' + state.defaults.updates + '"]');
    if (updates) { updates.checked = true; }
    $('space-free').textContent = state.defaults.free ? t('This disk has') + ' ' + state.defaults.free + ' ' + t('free.') : '';
    document.querySelectorAll('[data-next]').forEach(function (b) { b.addEventListener('click', function () { step(1); }); });
    document.querySelectorAll('[data-back]').forEach(function (b) { b.addEventListener('click', function () { step(-1); }); });
    document.querySelectorAll('input[name="account"]').forEach(function (radio) {
      radio.addEventListener('change', function () { $('join-only').hidden = accountKind() !== 'join'; nameChanged(); });
    });
    if (state.mode === 'setup' && state.account_here) {
      $('account-here').textContent = state.account_here;
      $('another').hidden = false;
      $('another-start').addEventListener('click', async function () {
        try {
          await api('POST', '/api/another', { name: $('another-name').value.trim() });
          $('another-said').textContent = t('The setup of the other account opened in a new page.');
        } catch (e) { $('another-said').textContent = e.message; }
      });
    }
    $('start').addEventListener('click', startSetup);
    $('restart').addEventListener('click', startSetup);
    $('retry').addEventListener('click', retry);
    $('finish').addEventListener('click', closePage);
    if (state.run.phase === 'running') {
      go('connection');
      following();
      return;
    }
    await loadPlan();
    go('welcome');
  }

  async function closePage() {
    try { await api('POST', '/api/quit'); } catch (e) { /* already closed */ }
    if (polling) { clearInterval(polling); polling = null; }
    $('progress-nav').hidden = true;
    showPage('closed');
    // Allowed only to a page a script opened; otherwise the "closed" page
    // says the tab can be closed.
    setTimeout(function () { window.close(); }, 1500);
  }

