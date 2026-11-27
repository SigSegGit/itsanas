  // ------------------------------------------------------------- settings

  const STATUS_WORDS = {
    healthy: [N('Running and up to date'), 'ok'],
    paused: [N('Paused: your files wait until you resume'), ''],
    // Back from a long silence: copies are being proved again, not lost.
    'catching-up': [N('Back after a long silence: catching up with your other machines'), ''],
    // A copied key image in use on two disks: this one writes nothing.
    twin: [N('Another disk holds this machine’s identity: it writes nothing until one of the two is set up afresh'), 'bad'],
    stale: [N('Running, but it has not synced for a while'), 'bad'],
    // MVP.md test L: the daemon runs, the person's files are not elsewhere yet.
    exposed: [N('Running, but your files are not safe yet: add a machine to this account so copies exist elsewhere'), 'bad'],
    short: [N('Running, but this disk can no longer hold what it promised to others'), 'bad'],
    // ARCHITECTURE §7: rounds that reach no other machine for hours, and a
    // vault that refuses the daemon's write probe.
    isolated: [N('Running, but it has not reached any of your other machines for hours: check that this machine is online'), 'bad'],
    unwritable: [N('Running, but this disk refuses every write: what it promised to others fails'), 'bad'],
    failing: [N('Running, but a machine failed a storage check: what it holds no longer counts as a copy'), 'bad'],
    lowdisk: [N('Running, but this disk is nearly full: free some room before writes start to fail'), 'bad'],
    stopped: [N('Not running'), 'bad'],
    departed: [N('This machine has left the network'), 'bad'],
    unknown: [N('Cannot tell right now'), ''],
  };
  const initial = {};
  let settingsTimer = null;

  function renderSettings(settings) {
    const word = STATUS_WORDS[settings.status] || STATUS_WORDS.unknown;
    show($('st-status'), word[0]);
    $('st-dot').className = 'dot ' + word[1];
    $('pause').hidden = settings.paused;
    $('pause-for').hidden = settings.paused;
    $('resume').hidden = !settings.paused;
    $('st-pledge-now').textContent = t('Now:') + ' ' + settings.pledge;
    $('st-update').hidden = !settings.update_notice;
    $('st-update').textContent = said(settings.update_notice || '');
    if (settings.vault) { delete $('st-vault').dataset.en; $('st-vault').textContent = settings.vault; }
    else { show($('st-vault'), N('the default place, next to the data of this machine')); }
    if (settings.job) {
      $('disk-said').textContent = (settings.job.state === 'failed' ? t('Failed:') + ' ' : '') + said(settings.job.said);
      document.querySelectorAll('[data-disk]').forEach(function (b) { b.disabled = settings.job.state === 'running'; });
    }
  }

  async function disk(form, sure) {
    if (!form.path) { $('disk-said').textContent = t('Choose a folder first.'); return; }
    if (sure && !window.confirm(t(sure))) { return; }
    try {
      const reply = await api('POST', '/api/disk', form);
      $('disk-said').textContent = said(reply.said);
    } catch (e) {
      $('disk-said').textContent = e.message;
    }
    refreshSettings();
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
      now.textContent = t('every') + ' ' + settings.interval + ' ' + t('(now)');
      $('interval').prepend(now);
      $('interval').value = '';
    }
  }

  async function refreshSettings() {
    try {
      const state = await api('GET', '/api/state');
      renderSettings(state.settings);
      await renderHosting();
      return state;
    } catch (e) {
      say(e.message);
      return null;
    }
  }

  async function control(form) {
    try {
      const reply = await api('POST', '/api/control', form);
      $('st-said').textContent = said(reply.said);
    } catch (e) {
      $('st-said').textContent = e.message;
    }
    refreshSettings();
  }

  // What the switch's section showed last, to send only a change.
  let hostingShown = '';

  function hostingForm() {
    return {
      contribute: $('st-contribute').checked ? 'on' : 'off',
      batteryFloor: $('st-floor').value.trim(),
      chargingMinutes: $('st-charging').value.trim(),
      meteredGb: $('st-allowance').value.trim(),
      meteredPeriod: $('st-period').value,
    };
  }

  async function renderHosting() {
    if (!hostingOffered) { return; }
    const values = await api('GET', '/api/hosting');
    // This runs every two seconds, and somebody may be typing: what they changed
    // stays on screen until Apply.
    if (hostingShown && JSON.stringify(hostingForm()) !== hostingShown) { return; }
    $('st-contribute').checked = values.contribute;
    $('st-floor').value = values.batteryFloor;
    $('st-charging').value = values.chargingMinutes;
    $('st-allowance').value = values.meteredGb;
    $('st-period').value = values.meteredPeriod;
    $('st-contribute-fields').disabled = !values.contribute;
    hostingShown = JSON.stringify(hostingForm());
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
    if (JSON.stringify(hostingForm()) !== hostingShown) {
      try {
        await api('POST', '/api/hosting', hostingForm());
        hostingShown = JSON.stringify(hostingForm());
        $('st-said').textContent = t('Saved.');
      } catch (e) {
        $('st-said').textContent = e.message;
        return;
      }
    }
    const form = settingsForm();
    if ($('st-pledge').value.trim() !== '' && !/^\d+$/.test($('st-pledge').value.trim())) {
      $('st-said').textContent = t('Type a whole number of GB.');
      return;
    }
    if (Object.keys(form).length === 0) {
      if (!$('st-said').textContent) { $('st-said').textContent = t('Nothing was changed.'); }
      return;
    }
    $('apply').disabled = true;
    try {
      await api('POST', '/api/run', form);
      $('st-said').textContent = t('ITSaNAS restarts (a few seconds).');
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
      $('st-run-step').textContent = t('Now:') + ' ' + t(STEPS.find(function (s) { return s.page === page; }).label);
    }
    if (run.phase === 'running') {
      setTimeout(settingsRun, 1000);
      return;
    }
    $('apply').disabled = false;
    $('st-run-step').textContent = '';
    if (run.phase === 'done') {
      $('st-said').textContent = state.settings.running ? t('Saved. ITSaNAS runs again.') : t('Saved.');
      $('st-pledge').value = '';
      $('st-invite').value = '';
      fillSettings(state.settings);
    } else {
      const failed = run.failed || { error: '', remedy: '' };
      $('st-said').textContent = t('Not saved:') + ' ' + failed.error + '\n' + t('What to do:') + ' ' + failed.remedy;
    }
  }

  // A job that asks in a native window (/api/forgot, /api/words, /api/passphrase):
  // started here, followed through /api/state, and never given a secret.
  async function forgot() {
    if (!window.confirm(N('Replace the keys on this machine with your 24 recovery words? A window asks for them, then for a new passphrase.'))) { return; }
    await windowJob('/api/forgot', 'st-forgot', 'A window asks for your 24 words, then a new passphrase.');
  }

  async function windowJob(route, buttonId, startedText) {
    const line = $(buttonId + '-said');
    $(buttonId).disabled = true;
    try {
      await api('POST', route);
    } catch (e) {
      $(buttonId).disabled = false;
      line.textContent = said(e.message);
      return;
    }
    line.textContent = t(startedText);
    for (;;) {
      await new Promise(function (resolve) { setTimeout(resolve, 1000); });
      const state = await refreshSettings();
      if (!state || state.run.phase === 'running') { continue; }
      $(buttonId).disabled = false;
      if (state.run.phase === 'done') {
        line.textContent = said(state.run.lines[state.run.lines.length - 1] || '');
      } else {
        const failed = state.run.failed || { error: '', remedy: '' };
        line.textContent = t('Not changed:') + ' ' + said(failed.error) + '\n' + t('What to do:') + ' ' + said(failed.remedy);
      }
      return;
    }
  }

  // ------------------------------------------------ devices of the account
  //
  // The list is the coordinator's (/api/devices); Withdraw sends the full id the
  // list gave (/api/withdraw), never this machine's own. The same list opens
  // under a setup that stopped at the device limit.
  async function showDevices(list, line, keepSaid) {
    if (!keepSaid) { line.textContent = ''; }
    let reply;
    try { reply = await api('GET', '/api/devices'); } catch (e) { line.textContent = e.message; return; }
    list.textContent = '';
    reply.devices.forEach(function (device) {
      const item = document.createElement('li');
      const label = document.createElement('span');
      label.textContent = device.short + (device.address ? ' \u00b7 ' + device.address : '') +
        (device.thisPhone ? ' \u00b7 ' + t('this machine') : '') +
        (device.dormant ? ' \u00b7 ' + t('resting, not heard from for over a week') : '') +
        (device.unconfirmed ? ' \u00b7 ' + t('named only by the coordinator; no signed record of it here') : '');
      item.appendChild(label);
      if (!device.thisPhone) {
        const button = document.createElement('button');
        button.type = 'button';
        button.textContent = t('Withdraw');
        button.addEventListener('click', function () { withdrawDevice(device, list, line); });
        item.appendChild(button);
      }
      list.appendChild(item);
    });
    if (!reply.complete && !keepSaid) {
      line.textContent = t('Only some of the devices could be listed: those heard from lately.');
    }
  }

  async function withdrawDevice(device, list, line) {
    if (!window.confirm(t('Withdraw this device? Nothing will dial it again. To use it again it must be set up afresh.') + '\n' + device.short)) { return; }
    let reply;
    try { reply = await api('POST', '/api/withdraw', { id: device.id }); } catch (e) { line.textContent = e.message; return; }
    await showDevices(list, line, true);
    line.textContent = said(reply.said);
  }

