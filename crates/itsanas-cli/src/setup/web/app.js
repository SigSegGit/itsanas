// ITSaNAS setup and settings page.
//
// Served by `itsanas setup` / `itsanas settings` on 127.0.0.1 only. This page
// never asks for, receives or sends a recovery word or a passphrase: those are
// asked in a separate native window, and the page only says so and waits.
// Every value from the server is shown with textContent, never as HTML.
'use strict';

(function () {
  const TOKEN_KEY = 'itsanas-page-key';
  let token = '';

  // The key arrives in the address's fragment (#t=...), which the browser
  // never sends to any server. It is moved out of the address bar at once,
  // so it is not left in history or on screen, and kept for this tab only.
  function readToken() {
    const match = /(?:^#|&)t=([0-9a-f]{32})(?:&|$)/.exec(location.hash);
    if (match) {
      token = match[1];
      try { sessionStorage.setItem(TOKEN_KEY, token); } catch (e) { /* private mode */ }
      history.replaceState(null, '', location.pathname);
    } else {
      try { token = sessionStorage.getItem(TOKEN_KEY) || ''; } catch (e) { token = ''; }
    }
  }

  async function api(method, path, form) {
    const init = {
      method: method,
      headers: { 'X-Itsanas-Token': token },
      cache: 'no-store',
      credentials: 'omit',
    };
    if (form) {
      init.headers['Content-Type'] = 'application/x-www-form-urlencoded';
      init.body = new URLSearchParams(form).toString();
    }
    let response;
    try {
      response = await fetch(path, init);
    } catch (e) {
      throw new Error('ITSaNAS stopped serving this page. Run the command again to reopen it.');
    }
    const data = await response.json().catch(function () { return {}; });
    if (!response.ok) {
      throw new Error(data.error || ('Refused (' + response.status + ').'));
    }
    return data;
  }

  const $ = function (id) { return document.getElementById(id); };

  // ------------------------------------------------------------- languages
  //
  // English is written in the page; other languages are i18n.js, keyed by the
  // English text. The system's language is the default, English when there
  // is no translation, and the list in the header changes it.
  const LANG_KEY = 'itsanas-lang';
  let dict = {};
  const original = new WeakMap();

  function t(text) { return dict[text] || text; }

  function chooseLanguage() {
    let lang = '';
    try { lang = localStorage.getItem(LANG_KEY) || ''; } catch (e) { lang = ''; }
    if (!lang) { lang = (navigator.language || 'en').slice(0, 2).toLowerCase(); }
    return (window.I18N && window.I18N[lang]) ? lang : 'en';
  }

  function translatePage(lang) {
    dict = (window.I18N && window.I18N[lang]) || {};
    document.documentElement.lang = lang;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!original.has(node)) { original.set(node, node.nodeValue); }
      const english = original.get(node);
      const key = english.trim();
      if (!key) { continue; }
      node.nodeValue = english.replace(key, t(key));
    }
    document.querySelectorAll('[placeholder],[aria-label]').forEach(function (el) {
      ['placeholder', 'aria-label'].forEach(function (name) {
        if (!el.hasAttribute(name)) { return; }
        const store = 'en' + name.replace('-', '');
        if (!el.dataset[store]) { el.dataset[store] = el.getAttribute(name); }
        el.setAttribute(name, t(el.dataset[store]));
      });
    });
  }

  function setupLanguage() {
    const lang = chooseLanguage();
    $('lang').value = lang;
    translatePage(lang);
    $('lang').addEventListener('change', function () {
      try { localStorage.setItem(LANG_KEY, $('lang').value); } catch (e) { /* private mode */ }
      translatePage($('lang').value);
      buildProgress();
      markProgress(current);
    });
  }

  function say(text) {
    const problem = $('problem');
    problem.textContent = text ? t(text) : '';
    problem.hidden = !text;
  }

  function showPage(name) {
    document.querySelectorAll('section[data-page]').forEach(function (section) {
      section.hidden = section.dataset.page !== name;
    });
    say('');
    const heading = document.querySelector('section[data-page="' + name + '"] h1');
    if (heading) { heading.focus(); }
  }

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
    STEPS.forEach(function (step) {
      const item = document.createElement('li');
      item.textContent = t(step.label);
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

  function step(delta) {
    const index = QUESTION_PAGES.indexOf(current);
    if (delta > 0 && !validate(current)) { return; }
    const next = QUESTION_PAGES[index + delta];
    if (next) { go(next); }
  }

  function accountKind() {
    const chosen = document.querySelector('input[name="account"]:checked');
    return chosen ? chosen.value : 'new';
  }

  function validate(page) {
    if (page === 'account' && !done.account && !$('username').value.trim()) {
      say('Choose a username.');
      $('username').focus();
      return false;
    }
    if (page === 'account' && !done.account && accountKind() === 'new' && nameTaken === 'yes') {
      say('This username is already taken. Choose another.');
      $('username').focus();
      return false;
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
  let nameTimer = null;

  async function checkName() {
    const username = $('username').value.trim();
    const coordinator = $('coordinator').value.trim();
    const line = $('username-check');
    nameTaken = 'unknown';
    line.textContent = '';
    if (!username || !coordinator || done.account) { return; }
    try {
      const reply = await api('POST', '/api/name', { username: username, coordinator: coordinator });
      if (username !== $('username').value.trim()) { return; }
      nameTaken = reply.taken;
      const isNew = accountKind() === 'new';
      if (reply.taken === 'yes') {
        line.textContent = isNew ? t('This username is already taken. Choose another.') : t('Account found.');
      } else if (reply.taken === 'no') {
        line.textContent = isNew ? t('This username is free.') : t('No account has this name on the network.');
      } else {
        line.textContent = t('The network cannot be asked right now; it is checked again before anything is made.');
      }
    } catch (e) { /* the engine asks again before any key is written */ }
  }

  function nameChanged() {
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
    slider.addEventListener('input', function () { $('pledge').value = slider.value; showEarns(); });
    $('pledge').addEventListener('input', function () {
      if (/^\d+$/.test($('pledge').value)) { slider.value = $('pledge').value; }
      showEarns();
    });
    showEarns();
  }

  async function browse(input) {
    try {
      const reply = await api('POST', '/api/pick', { start: input.value.trim() });
      if (reply.path) { input.value = reply.path; }
    } catch (e) { say(e.message); }
  }

  async function folderAction(path, pin, line) {
    try {
      const reply = await api('POST', pin ? '/api/pin' : '/api/open', path ? { path: path } : {});
      if (line) { line.textContent = reply.said; }
    } catch (e) { if (line) { line.textContent = e.message; } else { say(e.message); } }
  }

  function showDone(engine, said) {
    done[engine] = true;
    const note = document.querySelector('[data-done-for="' + engine + '"]');
    if (note) {
      note.textContent = 'Already done: ' + said.replace(/^done:\s*/, '');
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
    if (done.pledge) { $('pledge').value = ''; }
    if (done.folder) { $('folder').value = ''; }
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
    ['coordinator', 'invite', 'folder'].forEach(function (id) {
      const value = $(id).value.trim();
      if (value) { form[id] = value; }
    });
    if ($('pledge').value !== '') { form.pledge = $('pledge').value + 'G'; }
    const updates = document.querySelector('input[name="updates"]:checked');
    if (updates) { form.updates = updates.value; }
    form.background = $('background').checked ? 'yes' : 'no';
    return form;
  }

  async function startSetup() {
    if (current === 'folder' && !validate('folder')) { return; }
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
      mark.setAttribute('aria-label', finding.verdict);
      item.appendChild(mark);
      item.appendChild(document.createTextNode(finding.what));
      const detail = document.createElement('div');
      detail.className = 'detail';
      detail.textContent = finding.verdict === 'failed' && finding.remedy
        ? finding.detail + ' What to do: ' + finding.remedy
        : finding.detail;
      item.appendChild(detail);
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
        ? 'ITSaNAS runs on this machine, and its icon is near the clock. A welcome.txt file is in your folder: open it on your other machines to see that they sync.'
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
    $('coordinator').addEventListener('change', nameChanged);
    $('folder-browse').addEventListener('click', function () { browse($('folder')); });
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

  // ------------------------------------------------------------- settings

  const STATUS_WORDS = {
    healthy: ['Running and up to date', 'ok'],
    paused: ['Paused: your files wait until you resume', ''],
    stale: ['Running, but it has not synced for a while', 'bad'],
    stopped: ['Not running', 'bad'],
    departed: ['This machine has left the network', 'bad'],
    unknown: ['Cannot tell right now', ''],
  };
  const initial = {};
  let settingsTimer = null;

  function renderSettings(settings) {
    const word = STATUS_WORDS[settings.status] || STATUS_WORDS.unknown;
    $('st-status').textContent = t(word[0]);
    $('st-dot').className = 'dot ' + word[1];
    $('pause').hidden = settings.paused;
    $('pause-for').hidden = settings.paused;
    document.querySelector('label[for="pause-for"]').hidden = settings.paused;
    $('resume').hidden = !settings.paused;
    $('st-pledge-now').textContent = t('Now:') + ' ' + settings.pledge;
    $('st-update').hidden = !settings.update_notice;
    $('st-update').textContent = settings.update_notice || '';
  }

  function fillSettings(settings) {
    initial.folder = settings.folder || '';
    initial.coordinator = settings.coordinator || '';
    initial.updates = settings.updates || 'notify';
    $('st-updates').value = initial.updates;
    $('st-folder').value = initial.folder;
    $('st-coordinator').value = initial.coordinator;
    const option = Array.from($('interval').options).find(function (o) { return o.value === settings.interval; });
    if (!option && settings.interval !== 'auto') {
      const now = document.createElement('option');
      now.value = '';
      now.textContent = 'every ' + settings.interval + ' (now)';
      $('interval').prepend(now);
      $('interval').value = '';
    }
  }

  async function refreshSettings() {
    try {
      const state = await api('GET', '/api/state');
      renderSettings(state.settings);
      return state;
    } catch (e) {
      say(e.message);
      return null;
    }
  }

  async function control(form) {
    try {
      const reply = await api('POST', '/api/control', form);
      $('st-said').textContent = reply.said;
    } catch (e) {
      $('st-said').textContent = e.message;
    }
    refreshSettings();
  }

  function settingsForm() {
    const form = {};
    const pledge = $('st-pledge').value.trim();
    if (pledge !== '') { form.pledge = pledge + 'G'; }
    const folder = $('st-folder').value.trim();
    if (folder && folder !== initial.folder) { form.folder = folder; }
    const coordinator = $('st-coordinator').value.trim();
    if (coordinator && coordinator !== initial.coordinator) { form.coordinator = coordinator; }
    const invite = $('st-invite').value.trim();
    if (invite) { form.invite = invite; }
    if ($('st-updates').value !== initial.updates) { form.updates = $('st-updates').value; }
    return form;
  }

  async function apply() {
    const form = settingsForm();
    if ($('st-pledge').value.trim() !== '' && !/^\d+$/.test($('st-pledge').value.trim())) {
      $('st-said').textContent = 'Type a whole number of GB.';
      return;
    }
    if (Object.keys(form).length === 0) {
      $('st-said').textContent = 'Nothing was changed.';
      return;
    }
    $('apply').disabled = true;
    try {
      await api('POST', '/api/run', form);
      $('st-said').textContent = 'ITSaNAS restarts (a few seconds).';
      $('st-run').hidden = false;
      settingsRun();
    } catch (e) {
      $('st-said').textContent = e.message;
      $('apply').disabled = false;
    }
  }

  async function settingsRun() {
    const state = await refreshSettings();
    if (!state) { $('apply').disabled = false; return; }
    const run = state.run;
    renderWindow($('st-window-notice'), null, run.waiting);
    document.querySelectorAll('#st-run [data-secret-warning]').forEach(function (w) { w.hidden = !run.waiting; });
    if (run.step) {
      const page = pageOfEngine(run.step);
      $('st-run-step').textContent = 'Now: ' + STEPS.find(function (s) { return s.page === page; }).label;
    }
    if (run.phase === 'running') {
      setTimeout(settingsRun, 1000);
      return;
    }
    $('apply').disabled = false;
    $('st-run-step').textContent = '';
    if (run.phase === 'done') {
      $('st-said').textContent = state.settings.running ? 'Saved. ITSaNAS runs again.' : 'Saved.';
      $('st-pledge').value = '';
      $('st-invite').value = '';
      fillSettings(state.settings);
    } else {
      const failed = run.failed || { error: '', remedy: '' };
      $('st-said').textContent = 'Not saved: ' + failed.error + '\nWhat to do: ' + failed.remedy;
    }
  }

  async function signOut() {
    const sure = window.confirm(
      'Sign out of ITSaNAS on this machine?\n\n' +
      'Syncing stops, and this machine stops answering the other members\' checks until you sign in again. ' +
      'Your files and this machine\'s keys stay on this disk.');
    if (!sure) { return; }
    try {
      const reply = await api('POST', '/api/signout');
      $('st-said').textContent = reply.said;
    } catch (e) {
      $('st-said').textContent = e.message;
    }
    refreshSettings();
  }

  function initSettings(state) {
    document.title = 'ITSaNAS settings';
    $('mode-title').textContent = 'Settings';
    $('progress-nav').hidden = true;
    renderSettings(state.settings);
    fillSettings(state.settings);
    showPage('settings');
    $('pause').addEventListener('click', function () { control({ action: 'pause', for: $('pause-for').value }); });
    $('resume').addEventListener('click', function () { control({ action: 'resume' }); });
    $('sync-now').addEventListener('click', function () { control({ action: 'sync-now' }); });
    $('interval-save').addEventListener('click', function () {
      if ($('interval').value) { control({ action: 'interval', every: $('interval').value }); }
    });
    $('apply').addEventListener('click', apply);
    $('signout').addEventListener('click', signOut);
    $('close').addEventListener('click', closePage);
    $('st-folder-browse').addEventListener('click', function () { browse($('st-folder')); });
    $('st-open-folder').addEventListener('click', function () { folderAction($('st-folder').value.trim(), false, $('st-said')); });
    settingsTimer = setInterval(refreshSettings, 5000);
    if (state.run.phase === 'running') { $('st-run').hidden = false; settingsRun(); }
  }

  async function init() {
    setupLanguage();
    readToken();
    if (!token) {
      say('This page\'s key is missing. Open the address that the command printed in your terminal.');
      return;
    }
    let state;
    try { state = await api('GET', '/api/state'); } catch (e) { say(e.message); return; }
    if (state.mode === 'settings') { initSettings(state); } else { await initSetup(state); }
  }

  document.addEventListener('DOMContentLoaded', init);
  window.addEventListener('beforeunload', function () { if (settingsTimer) { clearInterval(settingsTimer); } });
}());
