  function initSettings(state) {
    document.title = 'ITSaNAS — ' + t('Settings');
    show($('mode-title'), 'Settings');
    document.body.classList.add('settings-mode');
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
    $('st-forgot').addEventListener('click', forgot);
    $('st-words').addEventListener('click', function () { windowJob('/api/words', 'st-words', 'A window asks for your passphrase, then shows your 24 words.'); });
    $('st-remove-check').addEventListener('click', removeCheck);
    $('st-remove-go').addEventListener('click', removeGo);
    $('st-remove-name').addEventListener('input', function () {
      $('st-remove-go').disabled = $('st-remove-name').value !== removeName;
    });
    $('st-delete-go').addEventListener('click', deleteGo);
    $('st-devices-show').addEventListener('click', function () { showDevices($('st-devices'), $('st-devices-said')); });
    $('st-peers-show').addEventListener('click', function () { peers(null, ''); });
    $('st-trash-show').addEventListener('click', function () { trash(null, []); });
    $('st-trash-delete').addEventListener('click', function () {
      const paths = chosenPaths();
      if (paths.length) { trash('delete', paths); } else { $('st-trash-said').textContent = t('Tick the files to delete first.'); }
    });
    $('st-trash-empty').addEventListener('click', function () { trash('empty', []); });
    $('st-del-save').addEventListener('click', function () { saveRules('here'); });
    $('st-del-all').addEventListener('click', function () { saveRules('all'); });
    $('st-del-follow').addEventListener('click', function () { saveRules('follow'); });
    $('st-g-trash').addEventListener('toggle', function () { if ($('st-g-trash').open) { loadRules(); loadTraces(); } });
    $('st-traces-slider').addEventListener('input', showTraceValue);
    $('st-traces-never').addEventListener('change', showTraceValue);
    $('st-traces-save').addEventListener('click', saveTraces);
    $('st-g-copies').addEventListener('toggle', function () { if ($('st-g-copies').open) { loadCopies(); } });
    $('st-copies-save').addEventListener('click', saveCopies);
    document.querySelectorAll('input[name="disk-use"]').forEach(function (box) {
      box.addEventListener('change', function () { diskUse(box.value); });
    });
    $('st-files-show').addEventListener('click', function () { files(null, ''); });
    // In the page only: the list is already here, so a long one is narrowed
    // without pausing the machine again.
    $('st-files-filter').addEventListener('input', function () {
      const wanted = $('st-files-filter').value.trim().toLowerCase();
      $('st-files').querySelectorAll('li').forEach(function (item) {
        item.hidden = wanted !== '' && (item.dataset.path || '').indexOf(wanted) < 0;
      });
    });
    $('st-autostart').addEventListener('change', function () { autostart($('st-autostart').checked); });
    if (state.features.background_choice) { autostart(null); }
    $('st-peer-add').addEventListener('click', function () {
      const address = $('st-peer-address').value.trim();
      if (address) { peers('add', address); }
    });
    $('st-passphrase').addEventListener('click', function () { windowJob('/api/passphrase', 'st-passphrase', 'A window asks for the current passphrase, then a new one.'); });
    $('close').addEventListener('click', closePage);
    $('st-folder-browse').addEventListener('click', function () { browse($('st-folder')); });
    $('st-open-folder').addEventListener('click', function () { folderAction($('st-folder').value.trim(), false, $('st-said')); });
    ['disk-move', 'disk-backup', 'disk-portable'].forEach(function (id) {
      $(id + '-browse').addEventListener('click', function () { browse($(id)); });
    });
    ['disk-move-go', 'disk-export', 'disk-restore', 'disk-portable-go'].forEach(function (id) { $(id).dataset.disk = ''; });
    $('disk-move-go').addEventListener('click', function () {
      disk({ action: 'move-vault', path: $('disk-move').value.trim(), same_disk: $('disk-same').checked ? 'true' : 'false' },
        N('Move the space for others there? ITSaNAS stops hosting for the time of the copy.'));
    });
    $('disk-export').addEventListener('click', function () { disk({ action: 'export', path: $('disk-backup').value.trim() }); });
    $('disk-restore').addEventListener('click', function () {
      disk({ action: 'restore', path: $('disk-backup').value.trim() }, N('Read this backup into this machine?'));
    });
    $('disk-portable-go').addEventListener('click', function () {
      disk({ action: 'portable', path: $('disk-portable').value.trim() },
        N('Write a portable ITSaNAS there? Whoever has the drive can try to guess your passphrase: use a long one.'));
    });
    initUsb();
    $('st-version').textContent = state.version;
    $('st-theme').value = state.theme;
    $('st-theme').addEventListener('change', function () {
      api('POST', '/api/theme', { theme: $('st-theme').value }).then(function () {
        if ($('st-theme').value === 'system') { document.documentElement.removeAttribute('data-theme'); }
        else { document.documentElement.setAttribute('data-theme', $('st-theme').value); }
      }).catch(function (e) { say(e.message); });
    });
    document.querySelectorAll('details[data-legal]').forEach(function (block) {
      block.addEventListener('toggle', function () {
        const text = block.querySelector('pre');
        if (!block.open || text.textContent) { return; }
        fetch(block.dataset.legal).then(function (r) { return r.text(); }).then(function (body) {
          text.textContent = body;
        }).catch(function (e) { text.textContent = e.message; });
      });
    });
    // Grey it at once, without forgetting a value: a disabled field keeps its own.
    $('st-contribute').addEventListener('change', function () {
      $('st-contribute-fields').disabled = !this.checked;
    });
    [['st-biometric', 'biometric'], ['st-metered', 'metered'], ['st-battery', 'battery']].forEach(function (pair) {
      const box = $(pair[0]);
      box.checked = state.host[pair[1]] === 'on';
      box.addEventListener('change', function () {
        api('POST', '/api/host', { name: pair[1], value: box.checked ? 'on' : 'off' }).catch(function (e) { say(e.message); });
      });
    });
    const room = Number(state.defaults.slider_max_gb) || 0;
    const slider = $('st-pledge-slider');
    function showShare() {
      const offered = parseInt($('st-pledge').value || slider.value, 10);
      $('st-pledge-earns').textContent = t('You offer') + ' ' + offered + ' GB → ' + t('you get about') + ' ' +
        Math.floor(offered * state.defaults.split_own / state.defaults.split_network) + ' GB ' + t('for your own files on the network.');
    }
    slider.max = String(room);
    slider.value = String(Math.min(room, state.settings.pledge_gb || 0));
    slider.addEventListener('input', function () { $('st-pledge').value = slider.value; showShare(); });
    $('st-pledge').addEventListener('input', function () {
      if (/^\d+$/.test($('st-pledge').value)) { slider.value = $('st-pledge').value; showShare(); }
    });
    showShare();
    $('st-pledge-same').addEventListener('click', async function () {
      try {
        const reply = await api('POST', '/api/space-default', {});
        if (reply.same_gb > 0) {
          slider.value = String(reply.same_gb);
          $('st-pledge').value = String(reply.same_gb);
          showShare();
          $('st-pledge-said').textContent = t('The same as your other machines.');
        } else if (reply.other_gb > 0) {
          $('st-pledge-said').textContent = t('Your other machines offer') + ' ' + reply.other_gb + ' GB. ' + t('This phone does not have room for that.');
        } else {
          $('st-pledge-said').textContent = t('No other machine offers space yet.');
        }
      } catch (e) { $('st-pledge-said').textContent = e.message; }
    });
    $('st-check-update').addEventListener('click', function () {
      api('POST', '/api/host', { name: 'update' })
        .then(function () { say(t('Asked. If a new version is out, it is installed and ITSaNAS restarts by itself.')); })
        .catch(function (e) { say(e.message); });
    });
    settingsTimer = setInterval(refreshSettings, 2000);
    if (state.run.phase === 'running') { $('st-run').hidden = false; settingsRun(); }
  }

  // The page is the same everywhere; what a shell lacks is not drawn, and the
  // routes behind it refuse (setup/web/features.rs).
  function applyFeatures(features) {
    document.querySelectorAll('[data-feature]').forEach(function (el) {
      if (!features[el.dataset.feature]) { el.setAttribute('data-off', ''); }
    });
    // A section whose every card is off would show its title over nothing:
    // an iPhone showed "Contribute to the network", empty.
    document.querySelectorAll('details.group').forEach(function (group) {
      const cards = group.querySelectorAll(':scope > section.card');
      const off = Array.prototype.every.call(cards, function (card) { return card.hasAttribute('data-off'); });
      if (cards.length > 0 && off) { group.setAttribute('data-off', ''); }
    });
    // Only a phone goes without the advanced settings (features.rs).
    onPhone = !features.advanced;
    hostingOffered = features.hosting;
    translatePage($('lang').value);
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
    applyFeatures(state.features);
    if (state.mode === 'settings') { initSettings(state); } else { await initSetup(state); }
  }

  document.addEventListener('DOMContentLoaded', init);
  window.addEventListener('beforeunload', function () {
    if (settingsTimer) { clearInterval(settingsTimer); }
    if (usbTimer) { clearInterval(usbTimer); }
  });
}());
