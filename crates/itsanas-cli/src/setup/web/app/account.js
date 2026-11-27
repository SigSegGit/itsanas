  // ------------------------------------------------------- start at login
  async function autostart(on) {
    const box = $('st-autostart');
    const line = $('st-autostart-said');
    let reply;
    try {
      reply = on === null ? await api('GET', '/api/autostart') : await api('POST', '/api/autostart', { on: String(on) });
    } catch (e) { line.textContent = e.message; return; }
    box.disabled = !reply.installed || reply.on === null;
    box.checked = reply.on === true;
    line.textContent = reply.installed ? said(reply.said) : t('No background service runs this account here: the setup offers one.');
  }

  // ------------------------------------------ machines to sync with (Advanced)
  async function peers(action, address) {
    const line = $('st-peers-said');
    let reply;
    try {
      reply = action ? await api('POST', '/api/peers', { action: action, address: address }) : await api('GET', '/api/peers');
    } catch (e) { line.textContent = e.message; return; }
    const list = $('st-peers');
    list.textContent = '';
    reply.peers.forEach(function (peer) {
      const item = document.createElement('li');
      const label = document.createElement('span');
      label.textContent = peer;
      item.appendChild(label);
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = t('Forget');
      button.addEventListener('click', function () { peers('forget', peer); });
      item.appendChild(button);
      list.appendChild(item);
    });
    line.textContent = said(reply.said);
    if (action === 'add') { $('st-peer-address').value = ''; }
  }

  // ------------------------------------------------------ the account itself
  //
  // Remove: a check first (one best-effort round, then what would be lost), the
  // account's name typed where pieces of the files exist only here. Delete: the
  // name typed. The server enforces both; the page only asks.
  let removeName = '';

  async function removeCheck() {
    const line = $('st-remove-said');
    line.textContent = t('Checking what is safe to remove...');
    $('st-remove-check').disabled = true;
    let reply;
    try { reply = await api('GET', '/api/remove'); } catch (e) {
      line.textContent = e.message;
      $('st-remove-check').disabled = false;
      return;
    }
    $('st-remove-check').disabled = false;
    line.textContent = '';
    removeName = reply.username;
    const only = reply.onlyHere > 0;
    $('st-remove-only').hidden = !only;
    $('st-remove-only').textContent = t('{n} of {total} pieces of your files exist only on this machine. Removing the account now destroys them for good. Sync with one of your other machines first.')
      .replace('{n}', reply.onlyHere).replace('{total}', reply.liveChunks);
    $('st-remove-reached').hidden = reply.reached > 0;
    $('st-remove-held').hidden = !(reply.heldForOthers > 0);
    $('st-remove-held').textContent = t('This machine keeps {size} for other members. They re-place it from their own copies.')
      .replace('{size}', (reply.heldForOthers / 1e9).toFixed(1) + ' GB');
    $('st-remove-name-box').hidden = !only;
    $('st-remove-name-label').textContent = t('Type {name} to remove it anyway').replace('{name}', reply.username);
    $('st-remove-name').value = '';
    $('st-remove-go').disabled = only;
    $('st-remove-sure').hidden = false;
  }

  async function removeGo() {
    const line = $('st-remove-said');
    $('st-remove-go').disabled = true;
    let reply;
    try { reply = await api('POST', '/api/remove', { confirm: $('st-remove-name').value }); } catch (e) {
      line.textContent = e.message;
      $('st-remove-go').disabled = false;
      return;
    }
    $('st-remove-sure').hidden = true;
    line.textContent = t('This account is removed from this machine.') +
      (reply.withdrawn ? '' : ' ' + t('This machine could not tell the network. It will be done at your next login.'));
  }

  async function deleteGo() {
    const line = $('st-delete-said');
    if (!window.confirm(t('Delete this account for good? This cannot be undone.'))) { return; }
    $('st-delete-go').disabled = true;
    try { await api('POST', '/api/delete', { confirm: $('st-delete-name').value.trim() }); } catch (e) {
      line.textContent = e.message;
      $('st-delete-go').disabled = false;
      return;
    }
    line.textContent = t('This account is deleted.');
  }

  async function signOut() {
    const sure = window.confirm(
      t('Sign out of ITSaNAS on this machine?') + '\n\n' +
      t('Syncing stops, and this machine stops answering the other members\' checks until you sign in again. Your files and this machine\'s keys stay on this disk.'));
    if (!sure) { return; }
    try {
      const reply = await api('POST', '/api/signout');
      $('st-said').textContent = said(reply.said);
    } catch (e) {
      $('st-said').textContent = e.message;
    }
    refreshSettings();
  }

