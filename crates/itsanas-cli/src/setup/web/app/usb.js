  // ------------------------------------------- copy this account to a key
  //
  // The page lists the drives (/api/keys), says for each whether the copy
  // fits and why not, and starts `itsanas copy-to-key PATH --detach`
  // (/api/copy-to-key), which asks both passphrases in windows of its own.
  // The page then follows the progress file the copy writes.

  const USB_WHY = {
    room: N('Too little room on it for the copy.'),
    account: N('This key already holds an ITSaNAS account: choose another key, or empty this one first.'),
    inside: N('This is where this account already lives: choose the key\'s own drive.'),
    missing: N('Nothing is there: is the key plugged in?'),
    path: N('Type a full path, starting with / or a drive letter.'),
  };
  const USB_WARN = {
    NTFS: N('NTFS: a Mac can read this key but not write to it, so the copy will not start on a Mac. Reformat it as exFAT to use it everywhere.'),
    FAT32: N('FAT32: no file on it may exceed 4 GB, and your account can grow past that. exFAT has no such limit.'),
  };
  // The steps the copy writes (copy_to_key.rs), in order.
  const USB_STEPS = [
    ['started', N('Copying in the background')],
    ['checking', N('Checking the key')],
    ['passphrase', N('Opening this machine\'s keys')],
    ['space', N('Checking the room on the key')],
    ['programs', N('Writing the programs for every system on the key')],
    ['account', N('Sealing the account on the key under its own passphrase')],
    ['enrolling', N('Enrolling the key with the coordinator')],
  ];
  let usbTimer = null;
  let usbBusy = false;
  let usbFollowing = false;
  let usbPathTimer = null;

  function usbButtons() {
    document.querySelectorAll('[data-usb-copy]').forEach(function (b) { b.disabled = usbBusy || b.dataset.fits !== 'true'; });
    $('st-usb-go').disabled = usbBusy || $('st-usb-go').dataset.fits !== 'true';
    $('st-usb-browse').disabled = usbBusy;
  }

  // Why a row cannot take the copy, in the page's language.
  function usbReason(key, sizes) {
    if (key.why === 'room') {
      return t(USB_WHY.room) + ' ' + (key.free ? key.free + ' ' + t('free,') + ' ' : '') +
        t('about') + ' ' + sizes.needed + ' ' + t('needed.');
    }
    return USB_WHY[key.why] ? t(USB_WHY[key.why]) : said(key.reason || '');
  }

  function usbFacts(key) {
    const facts = [];
    if (key.free && key.total) { facts.push(key.free + ' ' + t('free of') + ' ' + key.total); }
    else if (key.free) { facts.push(key.free + ' ' + t('free.')); }
    if (key.format) { facts.push(key.format); }
    if (key.external) { facts.push(t('external disk?')); }
    return facts.join(' · ');
  }

  function usbRow(key, sizes) {
    const item = document.createElement('li');
    const text = document.createElement('div');
    text.className = 'key-text';
    const name = document.createElement('strong');
    name.textContent = key.label;
    text.appendChild(name);
    const facts = document.createElement('span');
    facts.className = 'key-facts';
    facts.textContent = usbFacts(key);
    text.appendChild(facts);
    if (!key.fits) {
      const why = document.createElement('span');
      why.className = 'why';
      why.textContent = usbReason(key, sizes);
      text.appendChild(why);
    } else if (key.format && USB_WARN[key.format]) {
      const warn = document.createElement('span');
      warn.className = 'warn';
      warn.textContent = t(USB_WARN[key.format]);
      text.appendChild(warn);
    }
    item.appendChild(text);
    const go = document.createElement('button');
    go.type = 'button';
    go.className = 'primary';
    go.dataset.usbCopy = key.path;
    go.dataset.fits = String(key.fits);
    go.textContent = t('Copy here');
    go.setAttribute('aria-label', t('Copy here') + ': ' + key.label);
    go.addEventListener('click', function () { usbCopy(key.path); });
    item.appendChild(go);
    return item;
  }

  async function usbScan() {
    let reply;
    try { reply = await api('GET', '/api/keys'); } catch (e) { $('st-usb-said').textContent = e.message; return; }
    $('st-usb-need').textContent = t('The copy needs about') + ' ' + reply.needed + ' ' + t('on the key:') + ' ' +
      reply.programs + ' ' + t('for the programs') +
      (reply.account ? ', ' + reply.account + ' ' + t('for your files.') : '. ' + t('The size of your files is not known yet.'));
    const list = $('st-usb-keys');
    list.textContent = '';
    reply.keys.forEach(function (key) { list.appendChild(usbRow(key, reply)); });
    $('st-usb-none').hidden = reply.keys.length > 0;
    usbBusy = reply.copying;
    usbButtons();
    if (reply.copying && !usbFollowing) { usbFollow(); }
  }

  // "Another folder…": the same check as a row, for the path typed or chosen.
  async function usbCheckPath() {
    const path = $('st-usb-path').value.trim();
    const why = $('st-usb-path-why');
    $('st-usb-go').dataset.fits = 'false';
    why.textContent = '';
    why.className = 'why';
    if (path) {
      try {
        const reply = await api('POST', '/api/keys', { path: path });
        if (path !== $('st-usb-path').value.trim()) { return; }
        const key = reply.key;
        $('st-usb-go').dataset.fits = String(key.fits);
        if (!key.fits) { why.textContent = usbReason(key, reply); }
        else {
          why.className = 'warn';
          why.textContent = usbFacts(key) + (key.format && USB_WARN[key.format] ? ' — ' + t(USB_WARN[key.format]) : '');
        }
      } catch (e) { why.textContent = e.message; }
    }
    usbButtons();
  }

  async function usbCopy(path) {
    if (usbBusy) { return; }
    usbBusy = true;
    usbButtons();
    $('st-usb-said').textContent = '';
    $('st-usb-detail').textContent = '';
    try {
      await api('POST', '/api/copy-to-key', { path: path });
      usbFollow();
    } catch (e) {
      $('st-usb-said').textContent = e.message;
      usbBusy = false;
      usbButtons();
    }
  }

  function usbFollow() {
    usbFollowing = true;
    $('st-usb-run').hidden = false;
    if (!usbTimer) { usbTimer = setInterval(usbPoll, 1000); }
    usbPoll();
  }

  async function usbPoll() {
    let reply;
    try { reply = await api('GET', '/api/copy-to-key'); } catch (e) { $('st-usb-said').textContent = e.message; return; }
    const progress = reply.progress;
    const at = progress ? USB_STEPS.findIndex(function (s) { return s[0] === progress.step; }) : -1;
    $('st-usb-window').hidden = !(reply.running && !progress);
    if (reply.running) {
      $('st-usb-step').textContent = at >= 0 ? t(USB_STEPS[at][1]) + '…' : t('Waiting for the passphrase windows…');
      $('st-usb-progress').value = Math.max(0, at);
      return;
    }
    clearInterval(usbTimer);
    usbTimer = null;
    usbFollowing = false;
    $('st-usb-run').hidden = true;
    $('st-usb-step').textContent = '';
    if (progress && progress.done && !progress.error) {
      $('st-usb-said').textContent = t('The key is ready. Unplug it safely before taking it out.');
      $('st-usb-detail').textContent = said(progress.said);
    } else if (progress && progress.error) {
      $('st-usb-said').textContent = t('The copy stopped:') + ' ' + said(progress.error);
    } else {
      $('st-usb-said').textContent = t('The copy stopped without saying why. Try again; a key it left half written is refused and must be emptied first.');
    }
    usbBusy = false;
    usbScan();
  }

  function initUsb() {
    $('st-usb-refresh').addEventListener('click', usbScan);
    $('st-usb-browse').addEventListener('click', async function () {
      try {
        const reply = await api('POST', '/api/pick', { start: $('st-usb-path').value.trim() });
        if (reply.path) { $('st-usb-path').value = reply.path; usbCheckPath(); }
      } catch (e) { $('st-usb-said').textContent = e.message; }
    });
    $('st-usb-path').addEventListener('input', function () {
      $('st-usb-go').dataset.fits = 'false';
      usbButtons();
      if (usbPathTimer) { clearTimeout(usbPathTimer); }
      usbPathTimer = setTimeout(usbCheckPath, 400);
    });
    $('st-usb-go').addEventListener('click', function () {
      const path = $('st-usb-path').value.trim();
      if (path) { usbCopy(path); }
    });
    usbScan();
    if (section === 'key') {
      const card = $('st-usb');
      reveal(card);
      card.classList.add('highlight');
      card.scrollIntoView({ block: 'start' });
      $('st-h-usb').focus({ preventScroll: true });
      setTimeout(function () { card.classList.remove('highlight'); }, 4000);
    }
  }

