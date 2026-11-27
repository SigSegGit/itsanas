// ITSaNAS setup and settings page.
//
// Served by `itsanas setup` / `itsanas settings` on 127.0.0.1 only. This page
// never asks for, receives or sends a recovery word or a passphrase: those are
// asked in a separate native window, and the page only says so and waits.
// Every value from the server is shown with textContent, never as HTML.
// Served as one /app.js: the files of app/, one concern each, in the order
// web/mod.rs joins them, all inside the one function opened below.
'use strict';

(function () {
  const TOKEN_KEY = 'itsanas-page-key';
  let token = '';

  // The key arrives in the address's fragment (#t=...), which the browser
  // never sends to any server. It is moved out of the address bar at once,
  // so it is not left in history or on screen, and kept for this tab only.
  // The section the page opens at: `&section=key` after the key (the trays'
  // "Copy this account to a USB key"), or a plain `#key`.
  let section = '';

  function readToken() {
    const asked = /(?:^#|&)section=([a-z]+)(?:&|$)/.exec(location.hash);
    if (asked) { section = asked[1]; } else if (location.hash === '#key') { section = 'key'; }
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
      throw new Error(t('ITSaNAS stopped serving this page. Run the command again to reopen it.'));
    }
    const data = await response.json().catch(function () { return {}; });
    if (!response.ok) {
      throw new Error(data.error ? said(data.error) : t('Refused') + ' (' + response.status + ').');
    }
    return data;
  }

  const $ = function (id) { return document.getElementById(id); };

  // Open the section an element sits in, so it can be seen and used: the trays'
  // jump to a card, and anything that drives the page (its tests) by id.
  function reveal(element) {
    for (let node = element; node; node = node.parentElement) {
      if (node.tagName === 'DETAILS') { node.open = true; }
    }
  }
  window.itsanasReveal = function (id) { reveal($(id) || document.querySelector(id)); return true; };

  // ------------------------------------------------------------- languages
  //
  // English is written in the page; other languages are i18n.js, keyed by the
  // English text. The server says which in <html data-lang>: the one chosen
  // in the list before (kept in the node's home, since each page is a new
  // port and so a new, empty browser storage), else the browser's
  // Accept-Language. Without either, the browser's language here; English
  // when there is no translation. The list in the header changes it.
  let dict = {};
  const original = new WeakMap();

  // The page says "machine" and "window", which a person holding a phone does
  // not recognise: one set of sentences, with these words changed where it is
  // read on a phone (applyFeatures).
  const PHONE_WORDS = [
    [/\bSet up this machine\b/g, 'Set up this phone'],
    [/\bthis machine\b/g, 'this phone'],
    [/\bfirst machine\b/g, 'first device'],
    [/\ba separate ITSaNAS window\b/g, 'a separate ITSaNAS screen'],
    [/\bA separate ITSaNAS window\b/g, 'A separate ITSaNAS screen'],
    [/\bcette machine\b/g, 'ce téléphone'],
    [/\bCette machine\b/g, 'Ce téléphone'],
    [/\bvotre première machine\b/g, 'votre premier appareil'],
    [/\bma première machine\b/g, 'mon premier appareil'],
    [/\bune fenêtre ITSaNAS séparée\b/g, 'un écran ITSaNAS séparé'],
    [/\bUne fenêtre ITSaNAS séparée s’est ouverte\b/g, 'Un écran ITSaNAS séparé s’est ouvert'],
  ];
  let hostingOffered = true;
  let onPhone = false;

  function phoneWords(text) {
    return onPhone ? PHONE_WORDS.reduce(function (said, pair) { return said.replace(pair[0], pair[1]); }, text) : text;
  }

  function t(text) { return phoneWords(dict[text] || text); }
  // A text of a table, translated where it is shown: marks it for the test
  // that every text of the page has a French entry.
  function N(text) { return text; }

  // What the server says, line by line: its fixed sentences are in i18n.js,
  // the rest (a path, a name, a time) stays as it came.
  function said(text) { return String(text).split('\n').map(function (line) { return t(line); }).join('\n'); }

  // A text set by the script, kept in English so a change of language
  // translates it again.
  function show(el, english) {
    el.dataset.en = english;
    el.textContent = t(english);
  }

  function chooseLanguage() {
    const lang = document.documentElement.dataset.lang ||
      (navigator.language || 'en').slice(0, 2).toLowerCase();
    return (window.I18N && window.I18N[lang]) ? lang : 'en';
  }

  function translatePage(lang) {
    dict = (window.I18N && window.I18N[lang]) || {};
    document.documentElement.lang = lang;
    document.querySelectorAll('[data-en]').forEach(function (el) { el.textContent = t(el.dataset.en); });
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.parentElement && node.parentElement.dataset.en !== undefined) { continue; }
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
      // Kept by the server for the next page; a page without its key still
      // changes language, for this tab.
      api('POST', '/api/language', { lang: $('lang').value }).catch(function () { /* this tab only */ });
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

