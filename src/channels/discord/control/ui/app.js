/* Loduchand control panel. Vanilla JS, no build step.
 * Everything the server sends is rendered with textContent, never as HTML. */
'use strict';

(function () {
  // --- preferences (applied before the page paints) ------------------------------

  const PREF_KEY = 'loduchand.panel.prefs.v1';
  const ACCENTS = {
    iris: { label: 'Iris', color: '#8b93ff' },
    teal: { label: 'Teal', color: '#2fc4ae' },
    amber: { label: 'Amber', color: '#f2a93b' },
    rose: { label: 'Rose', color: '#f26d8f' },
    violet: { label: 'Violet', color: '#b38bff' },
    sky: { label: 'Sky', color: '#4ba9ff' },
  };
  const DEFAULT_PREFS = { theme: 'system', accent: 'iris', density: 'comfortable', pins: [], collapsed: {} };

  function loadPrefs() {
    try {
      const raw = localStorage.getItem(PREF_KEY);
      const saved = raw ? JSON.parse(raw) : {};
      return Object.assign({}, DEFAULT_PREFS, saved && typeof saved === 'object' ? saved : {});
    } catch (_) {
      return Object.assign({}, DEFAULT_PREFS);
    }
  }
  const prefs = loadPrefs();
  if (!Array.isArray(prefs.pins)) prefs.pins = [];
  if (!prefs.collapsed || typeof prefs.collapsed !== 'object') prefs.collapsed = {};

  function savePrefs() {
    try { localStorage.setItem(PREF_KEY, JSON.stringify(prefs)); } catch (_) { /* private mode */ }
  }

  const params = new URLSearchParams(location.search);
  const dark = window.matchMedia ? window.matchMedia('(prefers-color-scheme: dark)') : null;

  function applyPrefs() {
    const root = document.documentElement;
    const choice = params.get('theme') || prefs.theme;
    const theme = choice === 'light' || choice === 'dark' ? choice : (dark && !dark.matches ? 'light' : 'dark');
    root.dataset.theme = theme;
    root.dataset.density = prefs.density === 'compact' ? 'compact' : 'comfortable';
    const accent = ACCENTS[prefs.accent] || ACCENTS.iris;
    root.style.setProperty('--accent', accent.color);
  }
  applyPrefs();
  if (dark && dark.addEventListener) dark.addEventListener('change', applyPrefs);

  // --- small helpers ---------------------------------------------------------------

  const ICONS = {
    overview: '<path d="M4 5a1 1 0 0 1 1-1h5v7H4zM14 4h5a1 1 0 0 1 1 1v4h-6zM14 13h6v6a1 1 0 0 1-1 1h-5zM4 15h6v5H5a1 1 0 0 1-1-1z"/>',
    bell: '<path d="M6 8a6 6 0 1 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.9 1.9 0 0 0 3.4 0"/>',
    slash: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m14 8-4 8"/>',
    activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
    sliders: '<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0"/><circle cx="16" cy="6" r="2"/><circle cx="10" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>',
    search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
    pin: '<path d="M9 4h6l-1 6 3 3v2H7v-2l3-3z"/><path d="M12 15v6"/>',
    chevron: '<path d="m6 9 6 6 6-6"/>',
    right: '<path d="m9 6 6 6-6 6"/>',
    x: '<path d="M6 6l12 12M18 6 6 18"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>',
    up: '<path d="m6 15 6-6 6 6"/>',
    down: '<path d="m6 9 6 6 6-6"/>',
    restart: '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/>',
    logout: '<path d="M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3"/><path d="M10 17l-5-5 5-5M5 12h11"/>',
    sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
    moon: '<path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"/>',
    monitor: '<rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/>',
    hash: '<path d="M5 9h15M4 15h15M10 3 8 21M16 3l-2 18"/>',
    voice: '<path d="M11 5 6 9H3v6h3l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13"/>',
    user: '<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>',
    users: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0M16 4.5a3.5 3.5 0 0 1 0 7M18 14a6.5 6.5 0 0 1 3.5 6"/>',
    check: '<path d="m5 12 5 5 9-10"/>',
    alert: '<path d="M12 3 2 20h20z"/><path d="M12 10v4M12 17.5v.01"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.01"/>',
    menu: '<path d="M4 7h16M4 12h16M4 17h16"/>',
    shuffle: '<path d="M3 7h4l10 10h4M3 17h4l3-3M14 10l3-3h4M18 4l3 3-3 3M18 14l3 3-3 3"/>',
    repeat: '<path d="M4 11V9a3 3 0 0 1 3-3h12l-3-3M20 13v2a3 3 0 0 1-3 3H5l3 3"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    calendar: '<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M3 10h18M8 3v4M16 3v4"/>',
    edit: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>',
    server: '<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 7.5h.01M7 16.5h.01"/>',
    tag: '<path d="M3 12V4h8l10 10-8 8z"/><circle cx="7.5" cy="8.5" r="1.5"/>',
    message: '<path d="M4 5h16v11H9l-5 4z"/>',
    shield: '<path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z"/>',
    external: '<path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
    power: '<path d="M12 3v8M6.4 6.4a8 8 0 1 0 11.2 0"/>',
    eye: '<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
    reset: '<path d="M4 12a8 8 0 1 0 2.3-5.7"/><path d="M4 4v5h5"/>',
  };

  function icon(name, cls) {
    const span = document.createElement('span');
    span.className = 'icon' + (cls ? ' ' + cls : '');
    span.setAttribute('aria-hidden', 'true');
    // Static, trusted markup from the table above.
    span.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' + (ICONS[name] || '') + '</svg>';
    return span;
  }

  function h(tag, props) {
    const el = document.createElement(tag);
    if (props) {
      for (const k of Object.keys(props)) {
        const v = props[k];
        if (v === null || v === undefined || v === false) continue;
        if (k === 'class') el.className = v;
        else if (k === 'style') el.style.cssText = v;
        else if (k === 'dataset') Object.assign(el.dataset, v);
        else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2).toLowerCase(), v);
        else if (k === 'value') el.value = v;
        else if (k === 'checked' || k === 'disabled' || k === 'selected') el[k] = !!v;
        else if (v === true) el.setAttribute(k, '');
        else el.setAttribute(k, String(v));
      }
    }
    for (let i = 2; i < arguments.length; i++) append(el, arguments[i]);
    return el;
  }
  function append(el, kid) {
    if (kid === null || kid === undefined || kid === false) return;
    if (Array.isArray(kid)) { kid.forEach((k) => append(el, k)); return; }
    el.appendChild(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  }
  function clear(el) { while (el.firstChild) el.removeChild(el.firstChild); return el; }
  function $(sel, root) { return (root || document).querySelector(sel); }

  const IST = 'Asia/Kolkata';
  const fmtDateTime = new Intl.DateTimeFormat('en-GB', { timeZone: IST, day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hour12: false });
  const fmtFull = new Intl.DateTimeFormat('en-GB', { timeZone: IST, weekday: 'short', day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false });
  const fmtTime = new Intl.DateTimeFormat('en-GB', { timeZone: IST, hour: '2-digit', minute: '2-digit', hour12: false });
  const numberFmt = new Intl.NumberFormat('en-IN');

  function when(ts) { return fmtDateTime.format(new Date(ts * 1000)); }
  function ago(ts) {
    const s = Math.max(0, Math.round(Date.now() / 1000 - ts));
    if (s < 45) return 'just now';
    if (s < 3600) return Math.round(s / 60) + ' min ago';
    if (s < 86400) { const n = Math.round(s / 3600); return n + (n === 1 ? ' hour ago' : ' hours ago'); }
    const d = Math.round(s / 86400);
    return d === 1 ? 'yesterday' : d + ' days ago';
  }
  function duration(secs) {
    const d = Math.floor(secs / 86400), hh = Math.floor((secs % 86400) / 3600), m = Math.floor((secs % 3600) / 60);
    if (d) return d + 'd ' + hh + 'h';
    if (hh) return hh + 'h ' + m + 'm';
    if (m) return m + 'm';
    return Math.max(0, Math.floor(secs)) + 's';
  }
  function plural(n, one, many) { return n + ' ' + (n === 1 ? one : (many || one + 's')); }
  function hue(str) { let x = 0; for (const c of String(str)) x = (x * 31 + c.charCodeAt(0)) >>> 0; return x % 360; }
  /** "YYYY-MM-DDTHH:MM" in India time, for datetime-local inputs. */
  function toIstInput(rfc) {
    if (!rfc) return '';
    const d = new Date(rfc);
    if (isNaN(d)) return '';
    return new Date(d.getTime() + 330 * 60000).toISOString().slice(0, 16);
  }
  function fromIstInput(v) { return v ? v + ':00+05:30' : ''; }
  function istNowParts() {
    const d = new Date(Date.now() + 330 * 60000);
    return { minutes: d.getUTCHours() * 60 + d.getUTCMinutes(), day: Math.floor(d.getTime() / 86400000) };
  }
  function hm(mins) { mins = ((mins % 1440) + 1440) % 1440; return String(Math.floor(mins / 60)).padStart(2, '0') + ':' + String(mins % 60).padStart(2, '0'); }
  function parseHm(v) { const m = /^(\d{1,2}):(\d{2})$/.exec(String(v || '').trim()); if (!m) return null; const a = +m[1], b = +m[2]; return a < 24 && b < 60 ? a * 60 + b : null; }
  function isOn(v) { return /^(on|true|yes|1)$/i.test(String(v || '').trim()); }

  function avatar(url, name, cls) {
    const letter = (String(name || '?').trim()[0] || '?').toUpperCase();
    const fallback = () => h('span', { class: 'avatar ' + (cls || ''), style: 'background:hsl(' + hue(name) + ' 45% 42%)', 'aria-hidden': 'true' }, letter);
    if (!url) return fallback();
    const img = h('img', { class: 'avatar ' + (cls || ''), src: url, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' });
    img.addEventListener('error', () => img.replaceWith(fallback()), { once: true });
    return img;
  }

  // --- toasts, dialogs, popovers ------------------------------------------------------

  function toast(message, kind) {
    kind = kind || 'ok';
    const el = h('div', { class: 'toast ' + kind, role: kind === 'error' ? 'alert' : 'status' },
      icon(kind === 'error' ? 'alert' : kind === 'info' ? 'info' : 'check'), h('span', null, message));
    $('#toasts').appendChild(el);
    setTimeout(() => { el.classList.add('leaving'); setTimeout(() => el.remove(), 220); }, kind === 'error' ? 6000 : 3200);
  }

  const layers = []; // open overlays, newest last, each with close()

  function pushLayer(layer) { layers.push(layer); return layer; }
  function popLayer(layer) { const i = layers.indexOf(layer); if (i >= 0) layers.splice(i, 1); }

  function trapFocus(container, e) {
    const items = container.querySelectorAll('button:not(:disabled), [href], input:not(:disabled), select, textarea, [tabindex]:not([tabindex="-1"])');
    if (!items.length) return;
    const first = items[0], last = items[items.length - 1];
    if (e.shiftKey && document.activeElement === first) { last.focus(); e.preventDefault(); }
    else if (!e.shiftKey && document.activeElement === last) { first.focus(); e.preventDefault(); }
  }

  /** A yes/no dialog. Resolves true when confirmed. */
  function confirmDialog(opts) {
    return new Promise((resolve) => {
      const before = document.activeElement;
      const titleId = 'dlg-' + Math.random().toString(36).slice(2);
      let layer;
      const done = (answer) => { wrap.remove(); popLayer(layer); if (before && before.focus) before.focus(); resolve(answer); };
      const yes = h('button', { class: 'btn ' + (opts.danger ? 'danger-solid' : 'primary'), type: 'button', onclick: () => done(true) }, opts.confirm || 'Confirm');
      const modal = h('div', { class: 'modal', role: 'alertdialog', 'aria-modal': 'true', 'aria-labelledby': titleId },
        h('div', { class: 'modal-body' },
          h('h2', { id: titleId }, opts.icon ? icon(opts.icon, opts.danger ? 'danger' : 'warn') : null, opts.title),
          typeof opts.body === 'string' ? h('p', null, opts.body) : h('div', { class: 'body' }, opts.body)),
        h('div', { class: 'modal-foot' },
          h('button', { class: 'btn ghost', type: 'button', onclick: () => done(false) }, opts.cancel || 'Cancel'), yes));
      const wrap = h('div', { class: 'modal-wrap' }, h('div', { class: 'scrim', onclick: () => done(false) }), modal);
      modal.addEventListener('keydown', (e) => { if (e.key === 'Tab') trapFocus(modal, e); });
      layer = pushLayer({ close: () => done(false) });
      $('#layers').appendChild(wrap);
      yes.focus();
    });
  }

  /**
   * A searchable list anchored to a button. `load(query)` returns items
   * `{ id, label, group, lead, trail, sub }`, possibly asynchronously.
   */
  function openPicker(anchor, opts) {
    closePopovers();
    const input = h('input', { type: 'text', placeholder: opts.placeholder || 'Search', 'aria-label': opts.placeholder || 'Search', autocomplete: 'off', spellcheck: 'false' });
    const list = h('div', { class: 'pop-list', role: 'listbox' });
    const pop = h('div', { class: 'popover', role: 'dialog', 'aria-label': opts.title || 'Choose' },
      h('div', { class: 'search-box' }, icon('search'), input), list);
    let items = [];
    let active = 0;
    let seq = 0;
    let timer = null;

    const render = () => {
      clear(list);
      if (!items.length) { list.appendChild(h('div', { class: 'pop-empty' }, input.value ? 'Nothing matches “' + input.value + '”.' : (opts.empty || 'Nothing to pick.'))); return; }
      let group = null;
      items.forEach((it, i) => {
        if (it.group !== undefined && it.group !== group) { group = it.group; list.appendChild(h('div', { class: 'pop-group' }, group || 'No category')); }
        const btn = h('button', { type: 'button', class: 'pop-item', role: 'option', 'aria-selected': i === active ? 'true' : 'false',
          onmousemove: () => { if (active !== i) { active = i; mark(); } },
          onclick: () => pick(it) },
          it.lead || null, h('span', { class: 'grow' }, it.label, it.sub ? h('small', null, '  ' + it.sub) : null), it.trail || null);
        list.appendChild(btn);
      });
    };
    const mark = () => {
      list.querySelectorAll('.pop-item').forEach((el, i) => {
        el.setAttribute('aria-selected', i === active ? 'true' : 'false');
        if (i === active) el.scrollIntoView({ block: 'nearest' });
      });
    };
    const refresh = async () => {
      const mine = ++seq;
      const result = await opts.load(input.value.trim());
      if (mine !== seq) return;
      items = result || [];
      active = 0;
      render();
    };
    const pick = (it) => { close(); opts.onPick(it); };
    const close = () => { pop.remove(); popLayer(layer); document.removeEventListener('mousedown', outside, true); window.removeEventListener('resize', place); if (anchor && anchor.isConnected) anchor.focus(); };
    const outside = (e) => { if (!pop.contains(e.target) && !anchor.contains(e.target)) close(); };
    const place = () => {
      const r = anchor.getBoundingClientRect();
      const small = window.innerWidth <= 640;
      pop.classList.toggle('sheet', small);
      if (small) return;
      const width = Math.max(300, Math.min(360, r.width));
      pop.style.width = width + 'px';
      let left = Math.min(r.left, window.innerWidth - width - 8);
      left = Math.max(8, left);
      const below = window.innerHeight - r.bottom;
      pop.style.left = left + 'px';
      if (below < 300 && r.top > below) { pop.style.top = ''; pop.style.bottom = (window.innerHeight - r.top + 6) + 'px'; }
      else { pop.style.bottom = ''; pop.style.top = (r.bottom + 6) + 'px'; }
    };
    input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(refresh, opts.debounce || 0); });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') { active = Math.min(items.length - 1, active + 1); mark(); e.preventDefault(); }
      else if (e.key === 'ArrowUp') { active = Math.max(0, active - 1); mark(); e.preventDefault(); }
      else if (e.key === 'Enter') { if (items[active]) pick(items[active]); e.preventDefault(); }
      else if (e.key === 'Tab') { e.preventDefault(); }
    });
    const layer = pushLayer({ close, popover: true });
    $('#layers').appendChild(pop);
    place();
    window.addEventListener('resize', place);
    setTimeout(() => document.addEventListener('mousedown', outside, true), 0);
    input.focus();
    refresh();
  }
  function closePopovers() { layers.filter((l) => l.popover).forEach((l) => l.close()); }

  function openMenu(anchor, build) {
    closePopovers();
    const menu = h('div', { class: 'menu', role: 'menu' });
    build(menu, () => close());
    const close = () => { menu.remove(); popLayer(layer); anchor.setAttribute('aria-expanded', 'false'); document.removeEventListener('mousedown', outside, true); };
    const outside = (e) => { if (!menu.contains(e.target) && !anchor.contains(e.target)) close(); };
    const layer = pushLayer({ close, popover: true });
    $('#layers').appendChild(menu);
    const r = anchor.getBoundingClientRect();
    menu.style.top = (r.bottom + 6) + 'px';
    menu.style.right = Math.max(8, window.innerWidth - r.right) + 'px';
    anchor.setAttribute('aria-expanded', 'true');
    setTimeout(() => document.addEventListener('mousedown', outside, true), 0);
    const first = menu.querySelector('.menu-item');
    if (first) first.focus();
  }

  // --- API --------------------------------------------------------------------------

  class ApiError extends Error { constructor(msg, status) { super(msg); this.status = status; } }

  async function api(method, path, body) {
    const opts = { method, credentials: 'same-origin', headers: { Accept: 'application/json' } };
    if (method !== 'GET') opts.headers['X-Panel'] = '1';
    if (body !== undefined) { opts.headers['Content-Type'] = 'application/json'; opts.body = JSON.stringify(body); }
    let res;
    try { res = await fetch('/api' + path, opts); } catch (_) { throw new ApiError("Can't reach the bot. It may be restarting.", 0); }
    let data = null;
    try { data = await res.json(); } catch (_) { /* empty */ }
    if (res.status === 401 && path !== '/login' && S.booted) {
      renderSignIn({ error: 'Your session has ended. Run /panel in Discord to sign in again.' });
    }
    if (!res.ok) throw new ApiError((data && data.error) || 'Something went wrong (' + res.status + ').', res.status);
    return data;
  }

  // --- state --------------------------------------------------------------------------

  const S = {
    booted: false,
    me: null,
    status: null,
    statusAt: 0,
    sections: [],
    channels: [],
    roles: [],
    reminders: [],
    audit: [],
    members: new Map(),
    restartPending: false,
    fields: [], // editors on the current feature page
  };

  const chan = (id) => S.channels.find((c) => c.id === String(id));
  const role = (id) => S.roles.find((r) => r.id === String(id));
  const sectionById = (id) => S.sections.find((s) => s.id === id);

  async function memberById(id) {
    id = String(id);
    if (S.members.has(id)) return S.members.get(id);
    try {
      const m = await api('GET', '/discord/members/' + encodeURIComponent(id));
      S.members.set(id, m);
      return m;
    } catch (e) {
      if (e.status === 404) S.members.set(id, null);
      return null;
    }
  }
  function remember(list) { (list || []).forEach((m) => S.members.set(m.id, m)); return list; }

  async function loadAll() {
    const [me, status, catalog, channels, roles, reminders, audit] = await Promise.all([
      api('GET', '/me'), api('GET', '/status'), api('GET', '/catalog'), api('GET', '/discord/channels'),
      api('GET', '/discord/roles'), api('GET', '/reminders'), api('GET', '/audit?limit=300'),
    ]);
    S.me = me;
    S.status = status;
    S.statusAt = Date.now();
    S.sections = catalog.sections;
    S.channels = channels;
    S.roles = roles;
    S.reminders = reminders;
    S.audit = audit;
    S.members.set(me.id, { id: me.id, name: me.name, avatar: me.avatar });
  }
  async function refreshStatus() {
    try { S.status = await api('GET', '/status'); S.statusAt = Date.now(); renderSidebar(); } catch (_) { /* shown elsewhere */ }
  }
  async function refreshAudit() {
    try { S.audit = await api('GET', '/audit?limit=300'); } catch (_) { /* keep the old one */ }
  }

  // --- sign in ----------------------------------------------------------------------

  function renderSignIn(opts) {
    opts = opts || {};
    S.booted = false;
    layers.slice().forEach((l) => l.close());
    const root = clear($('#root'));
    root.removeAttribute('aria-busy');
    const card = h('main', { class: 'signin-card' },
      h('div', { class: 'signin-brand' }, h('span', { class: 'brand-mark' }, h('span', null, 'L')),
        h('div', null, h('strong', null, 'Loduchand'), h('small', null, 'MLCI control panel'))));
    if (opts.busy) {
      card.appendChild(h('h1', null, 'Signing you in…'));
      card.appendChild(h('div', { class: 'signin-busy' }, h('span', { class: 'spinner' }), 'Checking your link'));
    } else {
      card.appendChild(h('h1', null, opts.error ? "Let's get you a fresh link" : 'Sign in'));
      if (opts.error) {
        card.appendChild(h('div', { class: 'banner inline', role: 'alert' }, icon('alert'), h('p', null, opts.error)));
      } else {
        card.appendChild(h('p', { class: 'signin-lead' }, 'The panel has no passwords. Discord proves who you are.'));
      }
      card.appendChild(h('ol', { class: 'steps' },
        h('li', null, h('span', { class: 'step-n' }, '1'), h('span', null, 'In the MLCI server, run ', h('span', { class: 'slash' }, '/panel'), '. Only admins can.')),
        h('li', null, h('span', { class: 'step-n' }, '2'), h('span', null, h('b', null, 'Open the link'), ' in the private reply. It signs you in here straight away.')),
        h('li', null, h('span', { class: 'step-n' }, '3'), h('span', null, 'You stay signed in on this browser for 7 days.'))));
      card.appendChild(h('p', { class: 'signin-note' }, icon('shield'), h('span', null, 'Links work once and expire after 10 minutes. Never share one: whoever opens it can change the bot.')));
    }
    root.appendChild(h('div', { class: 'signin' }, card));
    document.title = 'Sign in · Loduchand';
  }

  // --- shell -----------------------------------------------------------------------

  let shell = null;

  function renderShell() {
    const root = clear($('#root'));
    root.removeAttribute('aria-busy');
    const searchInput = h('input', { type: 'search', id: 'global-search', placeholder: window.innerWidth < 520 ? 'Search' : 'Search settings, commands, features', 'aria-label': 'Search the panel', autocomplete: 'off', spellcheck: 'false' });
    const results = h('div', { class: 'search-results', role: 'listbox', id: 'search-results', hidden: true });
    const userBtn = h('button', { class: 'userbtn', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false', 'aria-label': 'Account' },
      avatar(S.me.avatar, S.me.name), h('span', { class: 'name' }, S.me.name), icon('chevron'));
    userBtn.addEventListener('click', () => openMenu(userBtn, (menu, close) => {
      append(menu, [
        h('div', { class: 'menu-head' }, h('b', null, S.me.name), h('span', null, 'Admin · signed in')),
        h('a', { class: 'menu-item', href: '#/settings', role: 'menuitem', onclick: close }, icon('sliders'), 'Panel settings'),
        h('button', { class: 'menu-item', type: 'button', role: 'menuitem', onclick: () => { close(); cycleTheme(); } }, icon(document.documentElement.dataset.theme === 'dark' ? 'sun' : 'moon'), document.documentElement.dataset.theme === 'dark' ? 'Light theme' : 'Dark theme'),
        h('button', { class: 'menu-item danger', type: 'button', role: 'menuitem', onclick: () => { close(); signOut(); } }, icon('logout'), 'Sign out'),
      ]);
    }));
    const menuBtn = h('button', { class: 'btn ghost icon-only menu-btn', type: 'button', 'aria-label': 'Open navigation', onclick: () => toggleNav(true) }, icon('menu'));

    shell = {
      app: h('div', { class: 'app' }),
      sidebar: h('aside', { class: 'sidebar', id: 'sidebar', 'aria-label': 'Navigation' }),
      banner: h('div', { id: 'banner' }),
      page: h('main', { class: 'content', id: 'page', tabindex: '-1' }),
      scrim: h('div', { class: 'scrim', hidden: true, onclick: () => toggleNav(false) }),
      search: searchInput,
      results,
    };
    append(shell.app, [
      shell.sidebar,
      shell.scrim,
      h('div', { class: 'main' },
        h('header', { class: 'topbar' },
          menuBtn,
          h('div', { class: 'search' }, h('label', { class: 'search-box' }, icon('search'), searchInput, h('kbd', { 'aria-hidden': 'true' }, '/')), results),
          h('div', { class: 'top-spacer' }),
          userBtn),
        shell.banner,
        shell.page),
    ]);
    root.appendChild(shell.app);
    setupSearch();
    renderSidebar();
    renderBanner();
  }

  function toggleNav(open) {
    shell.app.classList.toggle('nav-open', open);
    shell.scrim.hidden = !open;
    if (open) { const cur = shell.sidebar.querySelector('[aria-current="page"]') || shell.sidebar.querySelector('a'); if (cur) cur.focus(); }
  }

  function cycleTheme() {
    prefs.theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    params.delete('theme');
    savePrefs();
    applyPrefs();
    rerender();
  }

  async function signOut() {
    try { await api('POST', '/logout'); } catch (_) { /* signed out anyway */ }
    renderSignIn({});
    history.replaceState(null, '', '/');
  }

  function sectionStatus(id) {
    return S.status && S.status.sections ? S.status.sections.find((s) => s.id === id) : null;
  }

  function renderSidebar() {
    if (!shell) return;
    const side = clear(shell.sidebar);
    const guild = S.status && S.status.guild;
    const current = location.hash || '#/';

    const navItem = (href, lead, label, extra) => {
      const active = href === '#/' ? (current === '#/' || current === '#' || current === '' || current.startsWith('#/overview')) : current === href || current.startsWith(href + '/') || current.startsWith(href + '?');
      return h('a', { class: 'nav-item', href, 'aria-current': active ? 'page' : null, onclick: () => toggleNav(false) }, lead, h('span', { class: 'nav-text' }, label), extra || null);
    };
    const sectionItem = (sec) => {
      const st = sectionStatus(sec.id);
      const pinned = prefs.pins.includes(sec.id);
      const pin = h('button', { class: 'pin-btn', type: 'button', 'aria-pressed': pinned ? 'true' : 'false', 'aria-label': (pinned ? 'Unpin ' : 'Pin ') + sec.title,
        onclick: (e) => { e.preventDefault(); e.stopPropagation(); togglePin(sec.id); } }, icon('pin'));
      const tag = st && st.enabled === false ? h('span', { class: 'nav-tag' }, 'Off') : null;
      return navItem('#/s/' + sec.id, h('span', { class: 'nav-emoji', 'aria-hidden': 'true' }, sec.icon), sec.title, [tag, pin]);
    };

    side.appendChild(h('div', { class: 'side-brand' }, h('span', { class: 'brand-mark' }, h('span', null, 'L')),
      h('div', null, h('strong', null, 'Loduchand'), h('small', null, 'Control panel'))));
    if (guild) {
      side.appendChild(h('div', { class: 'side-server' }, avatar(guild.icon, guild.name), h('div', { style: 'min-width:0' }, h('b', null, guild.name), h('span', { class: 'srv-sub' }, numberFmt.format(guild.members) + ' members')),
        h('span', { class: 'online', 'data-tip': 'Connected to Discord', 'aria-label': 'Connected' })));
    }
    const nav = h('nav', { class: 'side-nav' });
    nav.appendChild(navItem('#/', icon('overview'), 'Overview'));

    const pinned = prefs.pins.map(sectionById).filter(Boolean);
    if (pinned.length) nav.appendChild(h('div', { class: 'nav-group' }, h('span', { class: 'nav-label' }, 'Pinned'), pinned.map(sectionItem)));
    const rest = S.sections.filter((s) => !prefs.pins.includes(s.id));
    if (rest.length) nav.appendChild(h('div', { class: 'nav-group' }, h('span', { class: 'nav-label' }, 'Features'), rest.map(sectionItem)));
    const active = S.reminders.filter((r) => r.enabled).length;
    nav.appendChild(h('div', { class: 'nav-group' }, h('span', { class: 'nav-label' }, 'Tools'),
      navItem('#/reminders', icon('bell'), 'Reminders', S.reminders.length ? h('span', { class: 'nav-count', 'aria-label': active + ' active' }, active + '/' + S.reminders.length) : null),
      navItem('#/commands', icon('slash'), 'Commands'),
      navItem('#/activity', icon('activity'), 'Activity log')));
    side.appendChild(nav);
    side.appendChild(h('div', { class: 'side-foot' }, navItem('#/settings', icon('sliders'), 'Panel settings'),
      h('div', { class: 'side-version' }, h('span', null, 'v' + (S.status ? S.status.bot.version : '')), h('span', null, 'India time'))));
  }

  function togglePin(id) {
    const i = prefs.pins.indexOf(id);
    if (i >= 0) prefs.pins.splice(i, 1); else prefs.pins.push(id);
    savePrefs();
    renderSidebar();
    const sec = sectionById(id);
    if (sec) toast(i >= 0 ? 'Unpinned ' + sec.title : 'Pinned ' + sec.title + ' to the top', 'info');
    if (route().name === 'settings' || route().name === 'section') rerender();
  }

  function renderBanner() {
    if (!shell) return;
    const b = clear(shell.banner);
    if (!S.restartPending) return;
    b.appendChild(h('div', { class: 'banner', role: 'status' }, icon('restart'),
      h('p', null, h('b', null, 'Some saved changes wait for a restart. '), h('span', null, 'They take effect the next time the bot starts.')),
      h('button', { class: 'btn sm', type: 'button', onclick: restartFlow }, 'Restart now…'),
      h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Dismiss', onclick: () => { S.restartPending = false; renderBanner(); } }, icon('x'))));
  }

  // --- global search -----------------------------------------------------------------

  function searchIndex() {
    const items = [];
    const pages = [
      ['Overview', '#/', 'overview', 'Status, switches and recent changes'],
      ['Reminders', '#/reminders', 'bell', 'Scheduled messages'],
      ['Commands', '#/commands', 'slash', 'Every slash command'],
      ['Activity log', '#/activity', 'activity', 'Who changed what'],
      ['Panel settings', '#/settings', 'sliders', 'Theme, accent, density, pins'],
    ];
    pages.forEach(([title, href, ic, sub]) => items.push({ kind: 'Pages', title, sub, href, lead: icon(ic), hay: title + ' ' + sub }));
    S.sections.forEach((sec) => {
      items.push({ kind: 'Features', title: sec.title, sub: sec.about, href: '#/s/' + sec.id, emoji: sec.icon, hay: sec.title + ' ' + sec.about });
      sec.settings.forEach((st) => items.push({ kind: 'Settings', title: st.label, sub: sec.title + ' · ' + st.help, href: '#/s/' + sec.id + '?k=' + encodeURIComponent(st.key), emoji: sec.icon, hay: st.label + ' ' + st.help + ' ' + st.key + ' ' + sec.title }));
      sec.commands.forEach((c) => items.push({ kind: 'Commands', title: '/' + c.name, sub: c.what, href: '#/commands?q=' + encodeURIComponent(c.name), lead: icon('slash'), hint: c.who, hay: c.name + ' ' + c.what + ' ' + c.usage + ' ' + sec.title }));
    });
    S.reminders.forEach((r) => items.push({ kind: 'Reminders', title: r.name, sub: scheduleWords(r.schedule), href: '#/reminders/' + r.id, lead: icon('bell'), hay: r.name + ' ' + r.lines.join(' ') }));
    return items;
  }

  function setupSearch() {
    const input = shell.search, box = shell.results;
    let found = [], active = 0;
    const close = () => { box.hidden = true; input.setAttribute('aria-expanded', 'false'); };
    const go = (it) => { close(); input.value = ''; input.blur(); navigate(it.href); };
    const draw = () => {
      clear(box);
      const q = input.value.trim();
      if (!q) { close(); return; }
      box.hidden = false;
      input.setAttribute('aria-expanded', 'true');
      if (!found.length) { box.appendChild(h('div', { class: 'search-empty' }, 'Nothing matches “' + q + '”.')); return; }
      let group = null;
      found.forEach((it, i) => {
        if (it.kind !== group) { group = it.kind; box.appendChild(h('div', { class: 'result-group' }, group)); }
        box.appendChild(h('div', { class: 'result', role: 'option', 'aria-selected': i === active ? 'true' : 'false',
          onmousedown: (e) => { e.preventDefault(); go(it); }, onmousemove: () => { if (active !== i) { active = i; mark(); } } },
          h('span', { class: 'result-icon' }, it.lead ? it.lead.cloneNode(true) : it.emoji),
          h('span', { style: 'min-width:0' }, h('div', { class: 'result-title' }, highlight(it.title, q)), h('div', { class: 'result-sub' }, it.sub)),
          it.hint ? h('span', { class: 'result-hint' }, it.hint) : h('span', { class: 'result-hint' }, icon('right'))));
      });
    };
    const mark = () => box.querySelectorAll('.result').forEach((el, i) => { el.setAttribute('aria-selected', i === active ? 'true' : 'false'); if (i === active) el.scrollIntoView({ block: 'nearest' }); });
    input.addEventListener('input', () => {
      const q = input.value.trim().toLowerCase();
      const words = q.split(/\s+/).filter(Boolean);
      const order = ['Pages', 'Features', 'Settings', 'Commands', 'Reminders'];
      found = searchIndex().map((it) => {
        const hay = it.hay.toLowerCase(), title = it.title.toLowerCase();
        if (!words.every((w) => hay.includes(w))) return null;
        const score = (title.startsWith(q) || title.startsWith('/' + q) ? 0 : title.includes(q) ? 1 : 2);
        return Object.assign({ score }, it);
      }).filter(Boolean)
        .sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind) || a.score - b.score)
        .slice(0, 30);
      // Best matches of each group first, groups in order of their best score.
      const best = {};
      found.forEach((it) => { best[it.kind] = Math.min(best[it.kind] === undefined ? 9 : best[it.kind], it.score); });
      found.sort((a, b) => best[a.kind] - best[b.kind] || order.indexOf(a.kind) - order.indexOf(b.kind) || a.score - b.score);
      active = 0;
      draw();
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') { active = Math.min(found.length - 1, active + 1); mark(); e.preventDefault(); }
      else if (e.key === 'ArrowUp') { active = Math.max(0, active - 1); mark(); e.preventDefault(); }
      else if (e.key === 'Enter') { if (found[active]) go(found[active]); e.preventDefault(); }
      else if (e.key === 'Escape') { input.value = ''; close(); input.blur(); }
    });
    input.addEventListener('focus', () => { if (input.value.trim()) draw(); });
    input.addEventListener('blur', () => setTimeout(close, 120));
  }

  function highlight(text, q) {
    const i = text.toLowerCase().indexOf(q.toLowerCase());
    if (!q || i < 0) return text;
    return [text.slice(0, i), h('mark', null, text.slice(i, i + q.length)), text.slice(i + q.length)];
  }

  // --- routing ----------------------------------------------------------------------

  function route() {
    const raw = location.hash.replace(/^#\/?/, '');
    const [path, query] = raw.split('?');
    const parts = path.split('/').filter(Boolean);
    const q = new URLSearchParams(query || '');
    const name = parts[0] === 's' ? 'section' : (parts[0] || 'overview');
    return { name, parts, q };
  }

  let currentHash = location.hash;
  let skipGuard = false;

  function navigate(href) { if (location.hash === href) rerender(); else location.hash = href; }

  function dirtyCount() { return S.fields.filter((f) => f.dirty()).length; }

  async function onHashChange() {
    if (!S.booted) return;
    const target = location.hash;
    const leavingSection = currentHash.startsWith('#/s/') && target.split('?')[0] !== currentHash.split('?')[0];
    if (!skipGuard && leavingSection && dirtyCount()) {
      history.replaceState(null, '', currentHash);
      const ok = await confirmDialog({ title: 'Leave without saving?', icon: 'alert', body: plural(dirtyCount(), 'unsaved change') + ' on this page will be lost.', confirm: 'Discard changes', danger: true });
      if (!ok) return;
      skipGuard = true;
      location.hash = target;
      return;
    }
    skipGuard = false;
    currentHash = target;
    rerender(true);
  }

  function rerender(scrollTop) {
    if (!S.booted || !shell) return;
    const r = route();
    layers.filter((l) => l.popover || l.drawer).forEach((l) => l.close());
    S.fields = [];
    renderSidebar();
    const page = clear(shell.page);
    removeSavebar();
    switch (r.name) {
      case 'section': renderSection(page, r.parts[1], r.q.get('k')); break;
      case 'reminders': renderReminders(page); if (r.parts[1]) openReminderEditor(r.parts[1]); break;
      case 'commands': renderCommands(page, r.q.get('q') || ''); break;
      case 'activity': renderActivity(page, r.q); break;
      case 'settings': renderSettings(page); break;
      default: renderOverview(page);
    }
    if (scrollTop && !r.q.get('k')) window.scrollTo(0, 0);
  }

  // --- shared pieces ------------------------------------------------------------------

  function pageHead(title, lead, actions, leading) {
    return h('div', { class: 'page-head' }, leading || null,
      h('div', { class: 'grow' }, h('div', { class: 'title-row' }, h('h1', null, title)), lead ? h('p', { class: 'lead' }, lead) : null),
      actions ? h('div', { class: 'page-actions' }, actions) : null);
  }

  function card(id, title, sub, body, opts) {
    opts = opts || {};
    const collapsed = !!prefs.collapsed[id];
    const bodyEl = h('div', { class: 'card-body' + (opts.pad ? ' pad' : ''), id: 'card-' + id });
    append(bodyEl, body);
    const btn = h('button', { class: 'collapse-btn', type: 'button', 'aria-expanded': collapsed ? 'false' : 'true', 'aria-controls': 'card-' + id, 'aria-label': (collapsed ? 'Expand ' : 'Collapse ') + title }, icon('chevron'));
    const el = h('section', { class: 'card' + (collapsed ? ' collapsed' : '') + (opts.cls ? ' ' + opts.cls : ''), 'aria-label': title },
      h('div', { class: 'card-head' }, h('div', { class: 'grow' }, h('h2', null, title), sub ? h('div', { class: 'sub' }, sub) : null), opts.actions || null, btn),
      bodyEl);
    btn.addEventListener('click', () => {
      const now = !el.classList.contains('collapsed');
      el.classList.toggle('collapsed', now);
      btn.setAttribute('aria-expanded', now ? 'false' : 'true');
      btn.setAttribute('aria-label', (now ? 'Expand ' : 'Collapse ') + title);
      if (now) prefs.collapsed[id] = true; else delete prefs.collapsed[id];
      savePrefs();
    });
    return el;
  }

  function switchEl(checked, label, onToggle, opts) {
    opts = opts || {};
    const btn = h('button', { class: 'switch' + (opts.small ? ' sm' : ''), type: 'button', role: 'switch', 'aria-checked': checked ? 'true' : 'false', 'aria-label': label },
      h('span', { class: 'track' }, h('span', { class: 'thumb' })), opts.noText ? null : h('span', { class: 'state', 'aria-hidden': 'true' }, checked ? 'On' : 'Off'));
    btn.addEventListener('click', () => onToggle(btn.getAttribute('aria-checked') !== 'true', btn));
    btn.set = (on) => { btn.setAttribute('aria-checked', on ? 'true' : 'false'); const s = btn.querySelector('.state'); if (s) s.textContent = on ? 'On' : 'Off'; };
    return btn;
  }

  function segmented(options, value, label, onChange) {
    const group = h('div', { class: 'segmented', role: 'radiogroup', 'aria-label': label });
    const buttons = options.map(([v, text, ic]) => h('button', { type: 'button', role: 'radio', 'aria-checked': v === value ? 'true' : 'false', tabindex: v === value ? '0' : '-1',
      onclick: () => select(v, true) }, ic ? icon(ic) : null, text));
    const select = (v, fire) => {
      buttons.forEach((b, i) => { const on = options[i][0] === v; b.setAttribute('aria-checked', on ? 'true' : 'false'); b.tabIndex = on ? 0 : -1; });
      if (fire) onChange(v);
    };
    group.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const i = buttons.findIndex((b) => b.getAttribute('aria-checked') === 'true');
      const n = (i + (e.key === 'ArrowRight' ? 1 : -1) + buttons.length) % buttons.length;
      select(options[n][0], true);
      buttons[n].focus();
      e.preventDefault();
    });
    append(group, buttons);
    return group;
  }

  function channelRef(id, opts) {
    const c = chan(id);
    const bare = opts && opts.bare;
    if (!c) return h('span', { class: 'inline-ref', title: 'Channel ' + id }, bare ? null : h('span', { class: 'glyph' }, '#'), 'unknown-channel');
    return h('span', { class: 'inline-ref' }, bare ? null : c.kind === 'voice' ? icon('voice') : h('span', { class: 'glyph' }, '#'), c.name, opts && opts.category && c.category ? h('small', { style: 'color:var(--faint);margin-left:4px' }, c.category) : null);
  }

  function channelItems(kind, selected) {
    const wanted = kind === 'voice' ? 'voice' : 'text';
    return (q) => {
      q = q.toLowerCase();
      return S.channels.filter((c) => c.kind === wanted && (!q || c.name.toLowerCase().includes(q) || (c.category || '').toLowerCase().includes(q)))
        .map((c) => ({ id: c.id, label: c.name, group: c.category || '', lead: wanted === 'voice' ? icon('voice') : h('span', { class: 'glyph' }, '#'),
          trail: selected && selected.includes(c.id) ? icon('check', 'check-mark') : null }));
    };
  }

  function roleItems(q) {
    q = q.toLowerCase();
    return S.roles.filter((r) => !q || r.name.toLowerCase().includes(q)).map((r) => ({
      id: r.id, label: r.name, sub: r.managed ? 'bot role' : '',
      lead: h('span', { class: 'role-dot', style: r.color ? 'background:' + r.color : '' }),
    }));
  }

  async function memberItems(q) {
    let list = [];
    try { list = remember(await api('GET', '/discord/members?q=' + encodeURIComponent(q))); } catch (e) { toast(e.message, 'error'); }
    return list.map((m) => ({ id: m.id, label: m.name, sub: m.username && m.username !== m.name ? '@' + m.username : '', lead: avatar(m.avatar, m.name, 'xs'), member: m }));
  }

  function memberChip(id, onRemove) {
    const chip = h('span', { class: 'chip' });
    const fill = (m) => {
      clear(chip);
      if (m) append(chip, [avatar(m.avatar, m.name, 'xs'), h('span', { class: 'chip-text' }, m.name)]);
      else { chip.classList.add('missing'); append(chip, [icon('user'), h('span', { class: 'chip-text' }, 'Unknown member ' + id)]); }
      if (onRemove) chip.appendChild(h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + (m ? m.name : id), onclick: onRemove }, icon('x')));
    };
    const cached = S.members.get(String(id));
    if (cached !== undefined) fill(cached);
    else { append(chip, h('span', { class: 'chip-text' }, '…')); memberById(id).then(fill); }
    return chip;
  }

  // --- overview -----------------------------------------------------------------------

  function tile(label, ic, value, sub) {
    return h('div', { class: 'tile' }, h('div', { class: 'tile-label' }, icon(ic), label), h('div', { class: 'tile-value' }, value), h('div', { class: 'tile-sub' }, sub));
  }

  function renderOverview(page) {
    document.title = 'Overview · Loduchand';
    const st = S.status;
    const guild = st.guild;
    const uptime = st.bot.uptime_secs + Math.floor((Date.now() - S.statusAt) / 1000);
    const activeReminders = S.reminders.filter((r) => r.enabled).length;
    const hour = +fmtTime.format(new Date()).slice(0, 2);
    const greeting = hour < 5 ? 'Up late' : hour < 12 ? 'Good morning' : hour < 17 ? 'Good afternoon' : 'Good evening';

    page.appendChild(pageHead(greeting + ', ' + S.me.name, 'Everything Loduchand does on ' + (guild ? guild.name : 'the server') + ', in one place.',
      h('button', { class: 'btn', type: 'button', onclick: restartFlow }, icon('restart'), 'Restart bot')));

    const upTile = tile('Uptime', 'clock', duration(uptime), 'since ' + when(st.bot.started));
    page.appendChild(h('div', { class: 'tiles' },
      tile('Version', 'tag', 'v' + st.bot.version, h('span', null, h('span', { class: 'ok' }, '● '), 'Online')),
      upTile,
      tile('Members', 'users', guild ? numberFmt.format(guild.members) : '—', guild ? guild.name : 'Server not loaded yet'),
      tile('Reminders', 'bell', activeReminders + ' active', plural(S.reminders.length, 'reminder') + ' in total')));
    clearInterval(renderOverview.timer);
    renderOverview.timer = setInterval(() => {
      if (!upTile.isConnected) { clearInterval(renderOverview.timer); return; }
      upTile.querySelector('.tile-value').textContent = duration(st.bot.uptime_secs + Math.floor((Date.now() - S.statusAt) / 1000));
    }, 15000);

    const switchable = st.sections.filter((s) => s.toggle_key).length;
    page.appendChild(h('div', { class: 'section-title' }, h('h2', null, 'Features'), h('span', null, switchable + ' with an on/off switch')));
    const grid = h('div', { class: 'features' });
    st.sections.forEach((s) => grid.appendChild(featureCard(s)));
    page.appendChild(grid);

    const changes = h('ul', { class: 'changes' });
    const recent = S.audit.slice(0, 6);
    if (!recent.length) changes.appendChild(h('li', { class: 'empty' }, icon('activity'), h('h3', null, 'No changes yet'), h('p', null, 'Changes made here show up with who made them.')));
    recent.forEach((e) => changes.appendChild(changeItem(e)));

    const restart = card('ov-restart', 'Restart the bot', null, h('div', { class: 'restart-body' },
      h('p', null, 'Needed for settings marked ', h('span', { class: 'badge restart' }, icon('restart'), 'Needs restart'), '. The bot is back in about 15 seconds.'),
      h('ul', { class: 'warn-list' },
        h('li', null, icon('alert'), 'Fights and battle royales in progress are cut off.'),
        h('li', null, icon('alert'), 'A running quiz loses its current question.')),
      h('div', null, h('button', { class: 'btn danger', type: 'button', onclick: restartFlow }, icon('power'), 'Restart bot…'))), { pad: true, cls: 'danger-zone' });

    page.appendChild(h('div', { class: 'two-col' },
      card('ov-changes', 'Recent changes', 'Newest first', [changes, S.audit.length ? h('div', { style: 'padding:10px var(--pad-card);border-top:1px solid var(--line)' }, h('a', { class: 'open-link', href: '#/activity' }, 'Full activity log', icon('right'))) : null]),
      restart));
  }

  function featureCard(s) {
    const sec = sectionById(s.id);
    const el = h('div', { class: 'feature' + (s.enabled === false ? ' is-off' : '') });
    const meta = plural(s.settings, 'setting') + ' · ' + plural(s.commands, 'command');
    el.appendChild(h('div', { class: 'feature-top' }, h('span', { class: 'emoji', 'aria-hidden': 'true' }, s.icon),
      h('div', { class: 'grow' }, h('a', { href: '#/s/' + s.id }, s.title), h('span', { class: 'meta' }, meta))));
    let foot;
    if (s.toggle_key) {
      const setting = sec && sec.settings.find((x) => x.key === s.toggle_key);
      const sw = switchEl(!!s.enabled, s.title + ' on or off', async (on, btn) => {
        if (!on || (setting && !setting.live)) {
          const ok = await confirmDialog({
            title: (on ? 'Turn on ' : 'Turn off ') + s.title + '?', icon: 'alert',
            body: on ? 'This setting only takes effect after a restart.' : 'Members lose ' + s.title + ' until it is switched back on. Anything of it running right now may stop.',
            confirm: on ? 'Turn on' : 'Turn off', danger: !on,
          });
          if (!ok) return;
        }
        btn.disabled = true;
        try {
          const updated = await api('PUT', '/settings/' + encodeURIComponent(s.toggle_key), { value: on ? 'on' : 'off' });
          replaceSetting(updated);
          s.enabled = on;
          btn.set(on);
          el.classList.toggle('is-off', !on);
          if (!updated.live) { S.restartPending = true; renderBanner(); }
          toast(s.title + (on ? ' is on' : ' is off'));
          refreshStatus();
          refreshAudit();
        } catch (e) { toast(e.message, 'error'); }
        btn.disabled = false;
      });
      foot = h('div', { class: 'feature-foot' }, sw, h('a', { class: 'open-link', href: '#/s/' + s.id }, 'Settings', icon('right')));
    } else {
      foot = h('div', { class: 'feature-foot' }, h('span', { class: 'no-switch' }, 'Always on'), h('a', { class: 'open-link', href: '#/s/' + s.id }, 'Settings', icon('right')));
    }
    el.appendChild(foot);
    return el;
  }

  function replaceSetting(updated) {
    S.sections.forEach((sec) => { const i = sec.settings.findIndex((x) => x.key === updated.key); if (i >= 0) sec.settings[i] = updated; });
  }

  // --- values in words -------------------------------------------------------------

  function formatValue(value, kind) {
    if (value === null || value === undefined) return h('span', { class: 'val none' }, '.env');
    if (value === '') return h('span', { class: 'val none' }, 'empty');
    const type = kind && kind.type;
    const wrap = (...kids) => h('span', { class: 'val' }, kids);
    switch (type) {
      case 'toggle': return wrap(isOn(value) ? 'On' : 'Off');
      case 'channel': case 'voice_channel': return wrap(channelRef(value));
      case 'channels': return wrap(value.split(',').filter(Boolean).map((id, i) => [i ? ', ' : '', channelRef(id.trim())]));
      case 'weighted_channels': return wrap(value.split(',').filter(Boolean).map((p, i) => { const [id, w] = p.split(':'); return [i ? ', ' : '', channelRef(id.trim()), ' ×' + (w || 1)]; }));
      case 'role': { const r = role(value); return wrap(h('span', { class: 'role-dot', style: r && r.color ? 'background:' + r.color : '' }), r ? r.name : 'role ' + value); }
      case 'users': {
        const ids = value.split(',').map((x) => x.trim()).filter(Boolean);
        const el = wrap(icon('users'), h('span', null, plural(ids.length, 'member') + '…'));
        Promise.all(ids.map((id) => memberById(id))).then((ms) => { el.lastChild.textContent = ms.map((m, i) => (m ? m.name : ids[i])).join(', '); });
        return el;
      }
      case 'choice': { const o = (kind.options || []).find((x) => x[0] === value); return wrap(o ? o[1] : value); }
      case 'number': case 'decimal': return wrap(value + (kind.unit ? ' ' + kind.unit : ''));
      default: return wrap(value.length > 60 ? value.slice(0, 57) + '…' : value);
    }
  }

  function oldValue(e) {
    const el = formatValue(e.old, e.kind);
    if (e.old !== null && e.old !== '') el.classList.add('old');
    return el;
  }

  function changeItem(e) {
    const who = e.user_name || 'Someone (' + e.user_id + ')';
    const diff = h('div', { class: 'change-diff' });
    if (e.key.startsWith('reminder:')) {
      diff.appendChild(h('span', { class: 'badge' }, e.change));
    } else if (e.section) {
      if (e.change === 'Reset to .env') append(diff, [h('span', { class: 'badge src-env' }, icon('reset'), 'Back to .env')]);
      else append(diff, [oldValue(e), h('span', { class: 'arrow', 'aria-label': 'to' }, '→'), formatValue(e.new, e.kind)]);
    }
    return h('li', { class: 'change' }, avatar(e.user_avatar, who),
      h('div', { style: 'min-width:0' }, h('div', { class: 'change-line' }, h('b', null, who), ' ', e.key.startsWith('reminder:') ? 'updated ' : 'changed ', h('b', null, e.label), e.section ? h('span', { style: 'color:var(--faint)' }, ' · ' + e.section.title) : null), diff),
      h('span', { class: 'change-time', title: fmtFull.format(new Date(e.ts * 1000)) + ' IST' }, ago(e.ts)));
  }

  // --- feature page ------------------------------------------------------------------

  function renderSection(page, id, focusKey) {
    const sec = sectionById(id);
    if (!sec) {
      page.appendChild(h('div', { class: 'empty' }, icon('alert'), h('h3', null, 'No such feature'), h('p', null, h('a', { href: '#/' }, 'Back to the overview'))));
      return;
    }
    document.title = sec.title + ' · Loduchand';
    const st = sectionStatus(sec.id);
    const pinned = prefs.pins.includes(sec.id);
    const stateBadge = st && st.enabled !== null && st.enabled !== undefined
      ? h('span', { class: 'badge ' + (st.enabled ? 'on' : 'off') }, h('span', { class: 'dot' }), st.enabled ? 'On' : 'Off') : null;
    const head = pageHead(sec.title, sec.about, [
      h('button', { class: 'btn', type: 'button', 'aria-pressed': pinned ? 'true' : 'false', onclick: () => togglePin(sec.id) }, icon('pin'), pinned ? 'Pinned' : 'Pin to sidebar'),
    ], h('span', { class: 'feature-icon', 'aria-hidden': 'true' }, sec.icon));
    const titleRow = head.querySelector('.title-row');
    if (stateBadge) titleRow.appendChild(stateBadge);
    page.appendChild(head);

    const live = sec.settings.filter((s) => s.live).length;
    const rows = h('div', null);
    if (!sec.settings.length) rows.appendChild(h('div', { class: 'empty' }, h('p', null, 'This feature has nothing to set.')));
    sec.settings.forEach((setting) => {
      const f = makeField(setting);
      S.fields.push(f);
      rows.appendChild(f.el);
    });
    page.appendChild(card('set-' + sec.id, 'Settings', sec.settings.length ? plural(sec.settings.length, 'setting') + ' · ' + (live === sec.settings.length ? 'all apply at once' : (sec.settings.length - live) + ' need a restart') : null, rows));

    const cmds = h('div', null);
    if (!sec.commands.length) cmds.appendChild(h('div', { class: 'empty' }, h('p', null, 'No slash commands.')));
    sec.commands.forEach((c) => cmds.appendChild(commandRow(c)));
    page.appendChild(card('cmd-' + sec.id, 'Commands', plural(sec.commands.length, 'slash command'), cmds));

    if (focusKey) {
      const f = S.fields.find((x) => x.setting.key === focusKey);
      if (f) {
        const c = f.el.closest('.card');
        if (c && c.classList.contains('collapsed')) c.querySelector('.collapse-btn').click();
        requestAnimationFrame(() => {
          f.el.scrollIntoView({ block: 'center' });
          f.el.classList.add('flash');
          setTimeout(() => f.el.classList.remove('flash'), 1400);
          const focusable = f.el.querySelector('.field-control input, .field-control button, .field-control select');
          if (focusable) focusable.focus({ preventScroll: true });
        });
      }
    }
  }

  function baseline(setting) {
    if (setting.value !== null && setting.value !== undefined) return setting.value;
    if (setting.origin === 'panel') return '';
    return setting.default || '';
  }

  function normalise(setting, v) {
    v = v === null || v === undefined ? '' : String(v);
    if (setting.kind.type === 'toggle') return isOn(v) ? 'on' : 'off';
    return v.trim();
  }

  function sourceBadge(setting) {
    const env = setting.env_value;
    const envText = env === null || env === undefined ? 'Not set in .env' : '.env value: ' + describePlain(env, setting.kind);
    if (setting.origin === 'panel') return h('span', { class: 'badge src-panel', tabindex: '0', 'data-tip': 'Saved in the panel. ' + envText }, 'Panel');
    if (setting.origin === 'env') return h('span', { class: 'badge src-env', tabindex: '0', 'data-tip': envText }, '.env');
    return h('span', { class: 'badge src-default', tabindex: '0', 'data-tip': setting.default ? 'Built-in default: ' + describePlain(setting.default, setting.kind) : 'Nothing set: the feature uses its own fallback' }, 'Default');
  }

  function describePlain(v, kind) {
    if (v === '') return 'empty';
    switch (kind.type) {
      case 'toggle': return isOn(v) ? 'On' : 'Off';
      case 'channel': case 'voice_channel': { const c = chan(v); return c ? '#' + c.name : v; }
      case 'channels': return v.split(',').map((id) => { const c = chan(id.trim()); return c ? '#' + c.name : id; }).join(', ');
      case 'weighted_channels': return v.split(',').map((p) => { const [id, w] = p.split(':'); const c = chan((id || '').trim()); return (c ? '#' + c.name : id) + ' ×' + (w || 1); }).join(', ');
      case 'role': { const r = role(v); return r ? '@' + r.name : v; }
      case 'choice': { const o = (kind.options || []).find((x) => x[0] === v); return o ? o[1] : v; }
      case 'number': case 'decimal': return v + (kind.unit ? ' ' + kind.unit : '');
      default: return v;
    }
  }

  function makeField(setting) {
    const f = { setting, draft: normalise(setting, baseline(setting)), error: null };
    const id = 'f-' + setting.key.replace(/[^A-Za-z0-9_]/g, '');
    const el = h('div', { class: 'field', id: 'field-' + setting.key });
    const control = h('div', { class: 'field-control' });
    const actions = h('div', { class: 'field-actions' });
    f.el = el;
    f.dirty = () => f.draft !== normalise(f.setting, baseline(f.setting));

    const drawInfo = () => {
      const s = f.setting;
      return h('div', { class: 'field-info' },
        h('div', { class: 'field-label' }, h('label', { for: id, id: id + '-label' }, s.label)),
        h('p', { class: 'field-help', id: id + '-help' }, s.help),
        h('div', { class: 'field-meta' },
          s.live ? h('span', { class: 'badge live', 'data-tip': 'A change applies straight away' }, h('span', { class: 'dot' }), 'Live')
            : h('span', { class: 'badge restart', 'data-tip': 'A change applies after the bot restarts' }, icon('restart'), 'Needs restart'),
          sourceBadge(s),
          h('span', { class: 'field-key' }, s.key)));
    };

    f.render = () => {
      clear(el);
      el.classList.toggle('dirty', f.dirty());
      clear(control);
      control.appendChild(renderControl(f, id, () => update()));
      el.appendChild(drawInfo());
      el.appendChild(h('div', null, control, actions));
      drawActions();
    };
    const drawActions = () => {
      clear(actions);
      const dirty = f.dirty();
      el.classList.toggle('dirty', dirty);
      f.error = dirty ? validateDraft(f.setting, f.draft) : null;
      const reset = f.setting.origin === 'panel'
        ? h('button', { class: 'btn sm ghost', type: 'button', 'data-tip': f.setting.env_value ? 'Use the .env value again: ' + describePlain(f.setting.env_value, f.setting.kind) : '.env has no value, so the default applies', onclick: () => save(null) }, icon('reset'), 'Reset to .env')
        : null;
      actions.hidden = !dirty && !reset;
      if (f.error) actions.appendChild(h('span', { class: 'error-text', role: 'alert', style: 'margin:0' }, icon('alert'), f.error));
      else if (dirty) actions.appendChild(h('span', { class: 'badge unsaved' }, h('span', { class: 'dot' }), 'Unsaved'));
      actions.appendChild(h('span', { class: 'grow' }));
      if (dirty) actions.appendChild(h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { f.draft = normalise(f.setting, baseline(f.setting)); f.render(); updateSavebar(); } }, 'Undo'));
      else if (reset) actions.appendChild(reset);
      if (dirty) actions.appendChild(h('button', { class: 'btn sm primary', type: 'button', disabled: !!f.error, onclick: () => save(f.draft) }, 'Save'));
    };
    const update = () => { drawActions(); updateSavebar(); };

    const save = async (value) => {
      try {
        const updated = await api('PUT', '/settings/' + encodeURIComponent(f.setting.key), { value });
        f.setting = updated;
        replaceSetting(updated);
        f.draft = normalise(updated, baseline(updated));
        f.render();
        updateSavebar();
        toast(value === null ? f.setting.label + ' is back to ' + (updated.origin === 'env' ? 'the .env value' : 'its default') : 'Saved ' + f.setting.label);
        if (!updated.live) { S.restartPending = true; renderBanner(); }
        if (updated.kind.type === 'toggle') refreshStatus().then(() => { if (route().name === 'section') { const b = shell.page.querySelector('.title-row .badge'); const st = sectionStatus(route().parts[1]); if (b && st && st.enabled !== null) { b.className = 'badge ' + (st.enabled ? 'on' : 'off'); b.lastChild.textContent = st.enabled ? 'On' : 'Off'; } } });
        refreshAudit();
        return true;
      } catch (e) {
        toast(f.setting.label + ': ' + e.message, 'error');
        return false;
      }
    };
    f.save = () => save(f.draft);
    f.render();
    return f;
  }

  function validateDraft(setting, v) {
    const k = setting.kind;
    if (k.type === 'number' || k.type === 'decimal') {
      if (v === '') return 'Needs a value.';
      const n = Number(v);
      if (!isFinite(n) || (k.type === 'number' && !Number.isInteger(n))) return k.type === 'number' ? 'Use a whole number.' : 'Use a number.';
      if (n < k.min || n > k.max) return 'Between ' + k.min + ' and ' + k.max + '.';
    }
    if (k.type === 'time' && v !== '' && parseHm(v) === null) return 'Use a time like 09:30.';
    if (k.type === 'weighted_channels' && v.split(',').filter(Boolean).some((p) => !chan(p.split(':')[0]))) return 'Pick a channel on every row.';
    return null;
  }

  function renderControl(f, id, changed) {
    const s = f.setting, k = s.kind;
    const set = (v, redraw) => { f.draft = v; if (redraw) { clear(f.el.querySelector('.field-control')).appendChild(renderControl(f, id, changed)); } changed(); };
    const described = id + '-help';
    switch (k.type) {
      case 'toggle':
        return switchEl(f.draft === 'on', s.label, (on, btn) => { btn.set(on); set(on ? 'on' : 'off'); });
      case 'channel':
      case 'voice_channel': {
        const voice = k.type === 'voice_channel';
        const c = f.draft ? chan(f.draft) : null;
        const btn = h('button', { class: 'picker-btn' + (f.draft && !c ? ' missing' : ''), type: 'button', id, 'aria-describedby': described, 'aria-haspopup': 'listbox' },
          f.draft ? (voice ? icon('voice') : h('span', { class: 'glyph' }, '#')) : icon(voice ? 'voice' : 'hash'),
          h('span', { class: 'value' + (f.draft ? '' : ' placeholder') }, c ? c.name : f.draft ? 'Unknown channel ' + f.draft : 'No channel set'),
          c && c.category ? h('small', { style: 'color:var(--faint)' }, c.category) : null, icon('chevron'));
        btn.addEventListener('click', () => openPicker(btn, { title: 'Choose a channel', placeholder: voice ? 'Search voice channels' : 'Search channels', load: channelItems(voice ? 'voice' : 'text', [f.draft]), onPick: (it) => set(it.id, true) }));
        return h('div', { class: 'picker-row' }, btn, f.draft ? h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Clear ' + s.label, 'data-tip': 'Clear', onclick: () => set('', true) }, icon('x')) : null);
      }
      case 'channels': {
        const ids = f.draft ? f.draft.split(',').filter(Boolean) : [];
        const chips = h('div', { class: 'chips', id, 'aria-describedby': described });
        ids.forEach((cid) => {
          const c = chan(cid);
          chips.appendChild(h('span', { class: 'chip' + (c ? '' : ' missing') }, h('span', { class: 'glyph' }, '#'), h('span', { class: 'chip-text' }, c ? c.name : 'unknown ' + cid),
            h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + (c ? c.name : cid), onclick: () => set(ids.filter((x) => x !== cid).join(','), true) }, icon('x'))));
        });
        const add = h('button', { class: 'btn sm', type: 'button' }, icon('plus'), ids.length ? 'Add' : 'Add channel');
        add.addEventListener('click', () => openPicker(add, { title: 'Add a channel', placeholder: 'Search channels', load: channelItems('text', ids), onPick: (it) => { if (!ids.includes(it.id)) set(ids.concat(it.id).join(','), true); } }));
        chips.appendChild(add);
        return chips;
      }
      case 'weighted_channels': return weightedEditor(f, id, set);
      case 'role': {
        const r = f.draft ? role(f.draft) : null;
        const btn = h('button', { class: 'picker-btn' + (f.draft && !r ? ' missing' : ''), type: 'button', id, 'aria-describedby': described, 'aria-haspopup': 'listbox' },
          f.draft ? h('span', { class: 'role-dot', style: r && r.color ? 'background:' + r.color : '' }) : icon('shield'),
          h('span', { class: 'value' + (f.draft ? '' : ' placeholder') }, r ? r.name : f.draft ? 'Unknown role ' + f.draft : 'No role set'), icon('chevron'));
        btn.addEventListener('click', () => openPicker(btn, { title: 'Choose a role', placeholder: 'Search roles', load: roleItems, onPick: (it) => set(it.id, true) }));
        return h('div', { class: 'picker-row' }, btn, f.draft ? h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Clear ' + s.label, onclick: () => set('', true) }, icon('x')) : null);
      }
      case 'users': {
        const ids = f.draft ? f.draft.split(',').filter(Boolean) : [];
        const chips = h('div', { class: 'chips', id, 'aria-describedby': described });
        ids.forEach((uid) => chips.appendChild(memberChip(uid, () => set(ids.filter((x) => x !== uid).join(','), true))));
        const add = h('button', { class: 'btn sm', type: 'button' }, icon('plus'), ids.length ? 'Add' : 'Add member');
        add.addEventListener('click', () => openPicker(add, { title: 'Add a member', placeholder: 'Search members by name', debounce: 180, empty: 'Type a name to search.', load: memberItems, onPick: (it) => { if (!ids.includes(it.id)) set(ids.concat(it.id).join(','), true); } }));
        chips.appendChild(add);
        return chips;
      }
      case 'number':
      case 'decimal': {
        const input = h('input', { class: 'input', type: 'number', id, inputmode: k.type === 'number' ? 'numeric' : 'decimal', min: k.min, max: k.max, step: k.type === 'number' ? '1' : 'any', value: f.draft, 'aria-describedby': described });
        input.addEventListener('input', () => { set(input.value.trim()); input.classList.toggle('invalid', !!validateDraft(s, input.value.trim())); });
        input.classList.toggle('invalid', !!validateDraft(s, f.draft));
        return h('div', null, h('div', { class: 'input-group' }, input, k.unit ? h('span', { class: 'addon' }, k.unit) : null), h('div', { class: 'hint' }, k.min + ' to ' + k.max + (s.default ? ' · default ' + s.default : '')));
      }
      case 'time': {
        const input = h('input', { class: 'input', type: 'time', id, value: f.draft, 'aria-describedby': described });
        input.addEventListener('input', () => set(input.value));
        return h('div', { class: 'input-group', style: 'max-width:220px' }, input, h('span', { class: 'addon' }, 'IST'));
      }
      case 'choice': {
        const opts = k.options || [];
        const short = opts.length <= 4 && opts.reduce((n, o) => n + o[1].length, 0) <= 36;
        if (short) return segmented(opts.map((o) => [o[0], o[1]]), f.draft, s.label, (v) => set(v));
        const sel = h('select', { class: 'select', id, 'aria-describedby': described }, opts.map((o) => h('option', { value: o[0], selected: o[0] === f.draft }, o[1])));
        sel.addEventListener('change', () => set(sel.value));
        return sel;
      }
      default: {
        const input = h('input', { class: 'input', type: 'text', id, value: f.draft, 'aria-describedby': described, spellcheck: 'false', placeholder: s.default ? 'Default: ' + s.default : 'Not set' });
        input.addEventListener('input', () => set(input.value));
        return input;
      }
    }
  }

  const WEIGHT_COLOURS = ['var(--accent)', '#4cc38a', '#e8ab4f', '#f26d8f', '#4ba9ff', '#b38bff', '#2fc4ae'];

  function weightedEditor(f, id, set) {
    const rows = (f.draft ? f.draft.split(',').filter(Boolean) : []).map((p) => { const [cid, w] = p.split(':'); return { id: (cid || '').trim(), w: Math.max(1, parseInt(w, 10) || 1) }; });
    const wrap = h('div', { class: 'weights', id });
    const total = rows.reduce((n, r) => n + r.w, 0) || 1;
    const commit = (redraw) => set(rows.map((r) => r.id + ':' + r.w).join(','), redraw);
    rows.forEach((r, i) => {
      const c = chan(r.id);
      const colour = WEIGHT_COLOURS[i % WEIGHT_COLOURS.length];
      const btn = h('button', { class: 'picker-btn' + (r.id && !c ? ' missing' : ''), type: 'button', 'aria-label': 'Channel for row ' + (i + 1) },
        h('span', { class: 'role-dot', style: 'background:' + colour }),
        h('span', { class: 'value' + (c ? '' : ' placeholder') }, c ? '#' + c.name : r.id ? 'unknown ' + r.id : 'Choose…'), icon('chevron'));
      btn.addEventListener('click', () => openPicker(btn, { title: 'Choose a channel', placeholder: 'Search channels', load: channelItems('text', rows.map((x) => x.id)), onPick: (it) => { if (rows.some((x, j) => j !== i && x.id === it.id)) { toast('#' + it.label + ' is already listed', 'error'); return; } r.id = it.id; commit(true); } }));
      const range = h('input', { class: 'range', type: 'range', min: '1', max: '20', value: Math.min(20, r.w), 'aria-label': 'Weight for ' + (c ? c.name : 'row ' + (i + 1)) });
      const num = h('input', { class: 'input', type: 'number', min: '1', max: '1000', value: r.w, 'aria-label': 'Weight number for ' + (c ? c.name : 'row ' + (i + 1)) });
      const share = h('span', { class: 'weight-share' }, Math.round((r.w / total) * 100) + '%');
      const sync = (w, from) => {
        r.w = Math.max(1, Math.min(1000, w || 1));
        if (from !== range) range.value = Math.min(20, r.w);
        if (from !== num) num.value = r.w;
        const t = rows.reduce((n, x) => n + x.w, 0) || 1;
        wrap.querySelectorAll('.weight-share').forEach((el, j) => { el.textContent = Math.round((rows[j].w / t) * 100) + '%'; });
        wrap.querySelectorAll('.weight-bar span').forEach((el, j) => { el.style.width = (rows[j].w / t) * 100 + '%'; });
        commit(false);
      };
      range.addEventListener('input', () => sync(parseInt(range.value, 10), range));
      num.addEventListener('input', () => sync(parseInt(num.value, 10), num));
      wrap.appendChild(h('div', { class: 'weight-row' }, btn, range, num, share,
        h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Remove row ' + (i + 1), onclick: () => { rows.splice(i, 1); commit(true); } }, icon('x'))));
    });
    if (rows.length > 1) {
      wrap.appendChild(h('div', { class: 'weight-bar', 'aria-hidden': 'true' }, rows.map((r, i) => h('span', { style: 'width:' + (r.w / total) * 100 + '%;background:' + WEIGHT_COLOURS[i % WEIGHT_COLOURS.length] }))));
    }
    const add = h('button', { class: 'btn sm', type: 'button' }, icon('plus'), 'Add channel');
    add.addEventListener('click', () => openPicker(add, { title: 'Add a channel', placeholder: 'Search channels', load: channelItems('text', rows.map((x) => x.id)), onPick: (it) => { if (rows.some((x) => x.id === it.id)) return; rows.push({ id: it.id, w: 1 }); commit(true); } }));
    wrap.appendChild(h('div', null, add, rows.length ? null : h('span', { class: 'hint', style: 'margin-left:10px' }, 'No channels: the feature uses its built-in ones.')));
    return wrap;
  }

  let savebar = null;
  function removeSavebar() { if (savebar) { savebar.remove(); savebar = null; } }
  function updateSavebar() {
    const n = dirtyCount();
    if (!n) { removeSavebar(); return; }
    if (!savebar) {
      savebar = h('div', { class: 'savebar', role: 'region', 'aria-label': 'Unsaved changes' });
      document.body.appendChild(savebar);
    }
    clear(savebar);
    const blocked = S.fields.some((f) => f.dirty() && f.error);
    append(savebar, [
      h('p', null, h('span', { class: 'dot' }), plural(n, 'unsaved change')),
      h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { S.fields.forEach((f) => { if (f.dirty()) { f.draft = normalise(f.setting, baseline(f.setting)); f.render(); } }); removeSavebar(); } }, 'Discard'),
      h('button', { class: 'btn sm primary', type: 'button', disabled: blocked, onclick: async () => {
        for (const f of S.fields.filter((x) => x.dirty())) { if (!(await f.save())) break; }
      } }, n > 1 ? 'Save all' : 'Save'),
    ]);
  }

  // --- commands ----------------------------------------------------------------------

  function whoKind(who) {
    const w = String(who || '').toLowerCase();
    if (w.includes('admin')) return 'admins';
    if (w.includes('captain')) return 'captains';
    if (w.includes('everyone') || w.includes('anyone')) return 'everyone';
    return 'other';
  }

  function commandRow(c) {
    const kind = whoKind(c.who);
    return h('div', { class: 'cmd' },
      h('div', { class: 'cmd-name' }, h('span', { class: 'sl' }, '/'), c.name),
      h('div', { class: 'cmd-body' }, h('div', { class: 'cmd-what' }, c.what), c.usage ? h('code', { class: 'cmd-usage' }, c.usage) : null),
      h('span', { class: 'badge who-' + kind }, kind === 'admins' ? icon('shield') : kind === 'captains' ? icon('tag') : icon('users'), c.who));
  }

  function renderCommands(page, initial) {
    document.title = 'Commands · Loduchand';
    const total = S.sections.reduce((n, s) => n + s.commands.length, 0);
    page.appendChild(pageHead('Commands', 'Every slash command Loduchand answers, by feature. Type one in Discord to use it.'));
    let who = 'all';
    const input = h('input', { type: 'search', placeholder: 'Filter commands', 'aria-label': 'Filter commands', value: initial });
    const count = h('span', { class: 'count', 'aria-live': 'polite' });
    const list = h('div', null);
    const draw = () => {
      clear(list);
      const q = input.value.trim().toLowerCase();
      let shown = 0;
      S.sections.forEach((sec) => {
        const cmds = sec.commands.filter((c) => (who === 'all' || whoKind(c.who) === who) && (!q || (c.name + ' ' + c.what + ' ' + c.usage + ' ' + sec.title).toLowerCase().includes(q)));
        if (!cmds.length) return;
        shown += cmds.length;
        const body = h('div', null, cmds.map(commandRow));
        const cardEl = card('cmds-' + sec.id, sec.title, plural(cmds.length, 'command'), body, { actions: h('a', { class: 'open-link', href: '#/s/' + sec.id }, 'Settings', icon('right')) });
        cardEl.querySelector('.card-head h2').prepend(h('span', { style: 'margin-right:8px', 'aria-hidden': 'true' }, sec.icon));
        list.appendChild(cardEl);
      });
      count.textContent = shown === total ? plural(total, 'command') : shown + ' of ' + total;
      if (!shown) list.appendChild(h('div', { class: 'card' }, h('div', { class: 'empty' }, icon('search'), h('h3', null, 'No commands match'), h('p', null, 'Try another word or who can use it.'))));
    };
    input.addEventListener('input', draw);
    page.appendChild(h('div', { class: 'toolbar' },
      h('label', { class: 'search-box' }, icon('search'), input),
      segmented([['all', 'All'], ['everyone', 'Everyone'], ['admins', 'Admins'], ['captains', 'Captains']], 'all', 'Who can use it', (v) => { who = v; draw(); }),
      count));
    page.appendChild(list);
    draw();
  }

  // --- activity ----------------------------------------------------------------------

  function renderActivity(page, q) {
    document.title = 'Activity log · Loduchand';
    page.appendChild(pageHead('Activity log', 'Every change made from the panel: who, what and when (India time).',
      h('button', { class: 'btn', type: 'button', onclick: async () => { await refreshAudit(); rerender(); toast('Up to date', 'info'); } }, icon('restart'), 'Refresh')));
    let section = q.get('section') || '';
    let key = q.get('key') || '';
    const secSel = h('select', { class: 'select', 'aria-label': 'Feature' });
    const keySel = h('select', { class: 'select', 'aria-label': 'Setting' });
    const body = h('div', null);
    const count = h('span', { class: 'count', 'aria-live': 'polite', style: 'margin-left:auto;color:var(--muted);font-size:13px' });

    const fillSelects = () => {
      clear(secSel);
      secSel.appendChild(h('option', { value: '' }, 'All features'));
      S.sections.forEach((s) => secSel.appendChild(h('option', { value: s.id, selected: s.id === section }, s.icon + '  ' + s.title)));
      secSel.appendChild(h('option', { value: 'reminders', selected: section === 'reminders' }, '⏰  Reminders'));
      clear(keySel);
      keySel.appendChild(h('option', { value: '' }, 'All settings'));
      const sec = sectionById(section);
      if (sec) sec.settings.forEach((s) => keySel.appendChild(h('option', { value: s.key, selected: s.key === key }, s.label)));
      keySel.disabled = !sec;
    };
    const draw = () => {
      clear(body);
      const rows = S.audit.filter((e) => (!section || (e.section && e.section.id === section)) && (!key || e.key === key));
      count.textContent = plural(rows.length, 'change');
      if (!rows.length) { body.appendChild(h('div', { class: 'empty' }, icon('activity'), h('h3', null, 'Nothing here yet'), h('p', null, section ? 'No changes to this yet.' : 'Changes made from the panel will show up here.'))); return; }
      const tbody = h('tbody');
      rows.forEach((e) => {
        const who = e.user_name || 'Unknown (' + e.user_id + ')';
        let diff;
        if (e.key.startsWith('reminder:')) diff = h('span', { class: 'badge' }, e.change);
        else if (!e.section) diff = h('span', { class: 'badge' }, 'Changed');
        else if (e.change === 'Reset to .env') diff = h('div', { class: 'diff' }, formatValue(e.old, e.kind), h('span', { class: 'arrow' }, '→'), h('span', { class: 'badge src-env' }, icon('reset'), 'Back to .env'));
        else diff = h('div', { class: 'diff' }, oldValue(e), h('span', { class: 'arrow', 'aria-label': 'to' }, '→'), formatValue(e.new, e.kind));
        tbody.appendChild(h('tr', null,
          h('td', { class: 'when', title: fmtFull.format(new Date(e.ts * 1000)) + ' IST' }, when(e.ts), h('small', null, ago(e.ts))),
          h('td', null, h('div', { class: 'who' }, avatar(e.user_avatar, who), who)),
          h('td', { class: 'what' }, h('b', null, e.label), h('small', null, e.section ? e.section.icon + ' ' + e.section.title : 'No longer in the panel')),
          h('td', { class: 'change-cell' }, diff)));
      });
      body.appendChild(h('div', { class: 'table-wrap' }, h('table', { class: 'log' },
        h('thead', null, h('tr', null, h('th', null, 'When'), h('th', null, 'Who'), h('th', null, 'What'), h('th', null, 'Change'))), tbody)));
    };
    secSel.addEventListener('change', () => { section = secSel.value; key = ''; fillSelects(); draw(); });
    keySel.addEventListener('change', () => { key = keySel.value; draw(); });
    fillSelects();
    page.appendChild(h('div', { class: 'filters' }, secSel, keySel, count));
    page.appendChild(h('div', { class: 'card' }, body));
    draw();
  }

  // --- panel settings ---------------------------------------------------------------

  function renderSettings(page) {
    document.title = 'Panel settings · Loduchand';
    page.appendChild(pageHead('Panel settings', 'How the panel looks for you. Saved in this browser only.'));

    const swatches = h('div', { class: 'swatches', role: 'radiogroup', 'aria-label': 'Accent colour' });
    Object.keys(ACCENTS).forEach((k) => {
      swatches.appendChild(h('button', { class: 'swatch', type: 'button', role: 'radio', 'aria-checked': prefs.accent === k ? 'true' : 'false', 'aria-label': ACCENTS[k].label, 'data-tip': ACCENTS[k].label, style: '--sw:' + ACCENTS[k].color,
        onclick: () => { prefs.accent = k; savePrefs(); applyPrefs(); rerender(); } }, icon('check')));
    });
    page.appendChild(card('prefs-look', 'Appearance', null, [
      h('div', { class: 'pref' }, h('div', null, h('b', null, 'Theme'), h('span', { class: 'help' }, 'System follows your device.')),
        segmented([['system', 'System', 'monitor'], ['dark', 'Dark', 'moon'], ['light', 'Light', 'sun']], params.get('theme') || prefs.theme, 'Theme', (v) => { prefs.theme = v; params.delete('theme'); savePrefs(); applyPrefs(); })),
      h('div', { class: 'pref' }, h('div', null, h('b', null, 'Accent colour'), h('span', { class: 'help' }, 'Used for switches, highlights and the selected page.')), swatches),
      h('div', { class: 'pref' }, h('div', null, h('b', null, 'Density'), h('span', { class: 'help' }, 'Compact fits more on the screen.')),
        segmented([['comfortable', 'Comfortable'], ['compact', 'Compact']], prefs.density, 'Density', (v) => { prefs.density = v; savePrefs(); applyPrefs(); })),
    ]));

    const pins = h('ul', { class: 'pins' });
    const pinned = prefs.pins.map(sectionById).filter(Boolean);
    const order = pinned.concat(S.sections.filter((s) => !prefs.pins.includes(s.id)));
    order.forEach((sec) => {
      const i = prefs.pins.indexOf(sec.id);
      const move = (d) => { const j = i + d; if (j < 0 || j >= prefs.pins.length) return; const t = prefs.pins[i]; prefs.pins[i] = prefs.pins[j]; prefs.pins[j] = t; savePrefs(); rerender(); const b = shell.page.querySelector('[data-pin="' + sec.id + '"][data-dir="' + d + '"]'); if (b && !b.disabled) b.focus(); };
      pins.appendChild(h('li', null, h('span', { class: 'emoji', 'aria-hidden': 'true' }, sec.icon), h('span', { class: 'grow' }, sec.title),
        i >= 0 ? h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Move ' + sec.title + ' up', disabled: i === 0, dataset: { pin: sec.id, dir: '-1' }, onclick: () => move(-1) }, icon('up')) : null,
        i >= 0 ? h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Move ' + sec.title + ' down', disabled: i === prefs.pins.length - 1, dataset: { pin: sec.id, dir: '1' }, onclick: () => move(1) }, icon('down')) : null,
        h('button', { class: 'btn sm' + (i >= 0 ? '' : ' ghost'), type: 'button', 'aria-pressed': i >= 0 ? 'true' : 'false', onclick: () => togglePin(sec.id) }, icon('pin'), i >= 0 ? 'Unpin' : 'Pin')));
    });
    page.appendChild(card('prefs-pins', 'Sidebar', 'Pinned features sit at the top of the sidebar, in this order.', pins));

    const expires = S.me.session_expires;
    page.appendChild(card('prefs-session', 'Session', null, [
      h('dl', { class: 'kv' },
        h('dt', null, 'Signed in as'), h('dd', null, h('span', { class: 'inline-ref' }, avatar(S.me.avatar, S.me.name, 'xs'), S.me.name), h('span', { class: 'field-key', style: 'margin-left:8px' }, S.me.id)),
        h('dt', null, 'Panel address'), h('dd', null, h('code', null, (S.status && S.status.panel_url) || location.origin)),
        h('dt', null, 'Session ends'), h('dd', null, expires ? fmtFull.format(new Date(expires * 1000)) + ' IST' : '—'),
        h('dt', null, 'Signing in'), h('dd', null, 'Run ', h('span', { class: 'slash' }, '/panel'), ' in Discord on any device.'),
        h('dt', null, 'Bot'), h('dd', null, 'Loduchand v' + S.status.bot.version + (S.status.guild ? ' on ' + S.status.guild.name : ''))),
      h('div', { style: 'padding:0 var(--pad-card) var(--pad-card)' }, h('button', { class: 'btn danger', type: 'button', onclick: signOut }, icon('logout'), 'Sign out')),
    ]));
  }

  // --- reminders --------------------------------------------------------------------

  function scheduleWords(s) {
    if (!s) return '';
    if (s.kind === 'every') {
      const m = s.minutes;
      if (m % 1440 === 0) return m === 1440 ? 'Every day' : m === 10080 ? 'Every week' : 'Every ' + m / 1440 + ' days';
      if (m % 60 === 0) return m === 60 ? 'Every hour' : 'Every ' + m / 60 + ' hours';
      return 'Every ' + m + ' minutes';
    }
    const times = s.times || [];
    return times.length ? 'Daily at ' + times.join(', ') : 'Daily (no times)';
  }

  function withinActive(r, mins) {
    const a = parseHm(r.active_from), b = parseHm(r.active_to);
    if (a === null || b === null) return true;
    return a <= b ? mins >= a && mins < b : mins >= a || mins < b;
  }

  /** Roughly when the next post goes out, in India time, or null. */
  function nextPost(r) {
    const now = istNowParts();
    const ends = r.ends ? new Date(r.ends).getTime() : null;
    for (let offset = 1; offset <= 8 * 1440; offset++) {
      const t = now.minutes + offset;
      const mins = t % 1440;
      let hit = false;
      if (r.schedule.kind === 'every') hit = r.schedule.minutes > 0 && t % r.schedule.minutes === 0;
      else hit = (r.schedule.times || []).some((x) => parseHm(x) === mins);
      if (hit && withinActive(r, mins)) {
        const at = Date.now() + offset * 60000;
        if (ends && at > ends) return null;
        const days = Math.floor(t / 1440);
        return { time: hm(mins), day: days === 0 ? 'today' : days === 1 ? 'tomorrow' : 'in ' + days + ' days', at };
      }
    }
    return null;
  }

  function fillLine(line, r, member) {
    const name = r.user_name || (member && member.name) || 'them';
    let hours = '?', days = '?';
    if (r.since) {
      const t = new Date(r.since).getTime();
      if (!isNaN(t)) { const hrs = Math.max(0, Math.floor((Date.now() - t) / 3600000)); hours = String(hrs); days = String(Math.floor(hrs / 24)); }
    }
    const out = [];
    const re = /\{(name|mention|hours|days)\}/g;
    let last = 0, m;
    while ((m = re.exec(line))) {
      if (m.index > last) out.push(line.slice(last, m.index));
      if (m[1] === 'mention') out.push(h('span', { class: 'mention' }, '@' + ((member && member.name) || r.user_name || 'member')));
      else out.push(m[1] === 'name' ? name : m[1] === 'hours' ? hours : days);
      last = re.lastIndex;
    }
    out.push(line.slice(last));
    return out;
  }

  /** A line with its placeholders marked, as written. */
  function templated(line) {
    return line.split(/(\{(?:name|mention|hours|days)\})/g).map((part, i) => (i % 2 ? h('span', { class: 'ph-inline' }, part) : part));
  }

  function renderReminders(page) {
    document.title = 'Reminders · Loduchand';
    page.appendChild(pageHead('Reminders', 'Messages Loduchand posts on a schedule. Switch one off to pause it without losing it.',
      h('a', { class: 'btn primary', href: '#/reminders/new' }, icon('plus'), 'New reminder')));
    const grid = h('div', { class: 'reminders' });
    S.reminders.forEach((r) => grid.appendChild(reminderCard(r)));
    grid.appendChild(h('a', { class: 'new-card', href: '#/reminders/new' }, icon('plus'), S.reminders.length ? 'New reminder' : 'Make your first reminder'));
    page.appendChild(grid);
  }

  function reminderCard(r) {
    const el = h('article', { class: 'reminder' + (r.enabled ? '' : ' is-off'), 'aria-label': r.name });
    const sw = switchEl(r.enabled, (r.enabled ? 'Pause ' : 'Resume ') + r.name, async (on, btn) => {
      btn.disabled = true;
      try {
        const updated = await api('POST', '/reminders/' + r.id + '/toggle');
        Object.assign(r, updated);
        el.replaceWith(reminderCard(r));
        renderSidebar();
        toast(r.name + (updated.enabled ? ' is running' : ' is paused'));
        refreshAudit();
      } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
    }, { noText: true });
    const facts = h('ul', { class: 'reminder-facts' },
      h('li', null, icon('hash'), h('span', null, channelRef(r.channel_id, { category: true, bare: true }))),
      h('li', null, icon(r.schedule.kind === 'every' ? 'repeat' : 'calendar'), h('span', null, scheduleWords(r.schedule), r.active_from ? h('span', { style: 'color:var(--faint)' }, ' · ' + r.active_from + '–' + r.active_to) : null)));
    if (r.user_id) {
      const li = h('li', null, icon('user'), h('span', null, '…'));
      facts.appendChild(li);
      memberById(r.user_id).then((m) => { const span = li.lastChild; clear(span); append(span, [h('span', { class: 'inline-ref' }, m ? avatar(m.avatar, m.name, 'xs') : null, r.user_name || (m ? m.name : r.user_id)), r.stop_when_back ? h('span', { style: 'color:var(--faint)' }, ' · stops when back') : null]); });
    }
    const next = r.enabled ? nextPost(r) : null;
    if (next) facts.appendChild(h('li', null, icon('clock'), h('span', null, 'Next ' + next.day + ' at ' + next.time)));
    if (r.ends) facts.appendChild(h('li', null, icon('power'), h('span', null, 'Ends ' + fmtFull.format(new Date(r.ends)))));
    el.appendChild(h('div', { class: 'reminder-head' },
      h('div', { class: 'grow' }, h('h3', null, r.name),
        h('div', { class: 'sub' }, r.enabled ? h('span', { class: 'badge on' }, h('span', { class: 'dot' }), 'Running') : h('span', { class: 'badge paused' }, 'Paused'),
          h('span', { class: 'badge' }, icon(r.order === 'random' ? 'shuffle' : 'repeat'), plural(r.lines.length, 'line')))),
      sw));
    el.appendChild(facts);
    if (r.lines[0]) el.appendChild(h('p', { class: 'reminder-quote', title: r.lines[0] }, templated(r.lines[0])));
    el.appendChild(h('div', { class: 'reminder-foot' },
      h('span', { class: 'grow' }, r.sent_count ? 'Sent ' + plural(r.sent_count, 'time') + (r.last_sent ? ' · last ' + ago(r.last_sent) : '') : 'Not sent yet'),
      h('a', { class: 'btn sm', href: '#/reminders/' + r.id }, icon('edit'), 'Edit')));
    return el;
  }

  function blankReminder() {
    const general = S.channels.find((c) => c.kind === 'text' && c.name === 'general');
    return { id: 0, name: '', enabled: true, channel_id: general ? general.id : '', lines: [''], order: 'rotate', schedule: { kind: 'every', minutes: 60 },
      active_from: '', active_to: '', user_id: '', user_name: '', since: '', stop_when_back: false, welcome_line: '', ends: '', last_sent: 0, sent_count: 0 };
  }

  function openReminderEditor(which) {
    const existing = which === 'new' ? null : S.reminders.find((r) => String(r.id) === String(which));
    if (which !== 'new' && !existing) { toast('That reminder no longer exists', 'error'); history.replaceState(null, '', '#/reminders'); currentHash = '#/reminders'; return; }
    const d = JSON.parse(JSON.stringify(existing || blankReminder()));
    if (!d.lines.length) d.lines = [''];
    const original = JSON.stringify(d);
    let member = d.user_id ? S.members.get(d.user_id) || null : null;
    let previewIndex = d.lines.length ? d.sent_count % d.lines.length : 0;
    let everyUnit = d.schedule.kind === 'every' ? (d.schedule.minutes % 1440 === 0 ? 'days' : d.schedule.minutes % 60 === 0 ? 'hours' : 'minutes') : 'hours';
    const before = document.activeElement;

    const titleId = 'drawer-title';
    const body = h('div', { class: 'drawer-body' });
    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, existing ? 'Save changes' : 'Create reminder');
    const drawer = h('div', { class: 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': titleId },
      h('div', { class: 'drawer-head' }, h('div', { class: 'grow' }, h('h2', { id: titleId }, existing ? 'Edit reminder' : 'New reminder'),
        h('div', { class: 'sub' }, existing ? (existing.sent_count ? 'Sent ' + plural(existing.sent_count, 'time') + (existing.last_sent ? ', last ' + ago(existing.last_sent) : '') : 'Not sent yet') : 'Posts on its own once saved and switched on')),
        h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Close', onclick: () => attemptClose() }, icon('x'))),
      body,
      h('div', { class: 'drawer-foot' },
        existing ? h('button', { class: 'btn danger', type: 'button', onclick: remove }, icon('trash'), 'Delete') : null,
        h('span', { class: 'grow' }),
        h('button', { class: 'btn ghost', type: 'button', onclick: () => attemptClose() }, 'Cancel'),
        saveBtn));
    const scrim = h('div', { class: 'scrim', onclick: () => attemptClose() });
    const layer = pushLayer({ drawer: true, close: () => finish(true), attempt: () => attemptClose() });
    drawer.addEventListener('keydown', (e) => { if (e.key === 'Tab' && !layers.some((l) => l.popover)) trapFocus(drawer, e); });
    $('#layers').appendChild(scrim);
    $('#layers').appendChild(drawer);

    function finish(silent) {
      drawer.remove(); scrim.remove(); popLayer(layer);
      if (silent) return;
      if (location.hash.startsWith('#/reminders/')) { history.replaceState(null, '', '#/reminders'); currentHash = '#/reminders'; }
      if (before && before.isConnected && before.focus) before.focus();
    }
    async function attemptClose() {
      if (JSON.stringify(d) !== original) {
        const ok = await confirmDialog({ title: 'Discard this reminder’s changes?', icon: 'alert', body: 'What you changed here has not been saved.', confirm: 'Discard', danger: true });
        if (!ok) return;
      }
      finish();
    }
    async function remove() {
      const ok = await confirmDialog({ title: 'Delete “' + existing.name + '”?', icon: 'trash', danger: true, body: 'It stops posting and its lines are gone for good. To stop it for a while, switch it off instead.', confirm: 'Delete reminder' });
      if (!ok) return;
      try {
        await api('DELETE', '/reminders/' + existing.id);
        S.reminders = S.reminders.filter((r) => r.id !== existing.id);
        finish();
        rerender();
        toast('Deleted ' + existing.name);
        refreshAudit();
      } catch (e) { toast(e.message, 'error'); }
    }
    saveBtn.addEventListener('click', async () => {
      const problem = localCheck();
      if (problem) { toast(problem, 'error'); return; }
      saveBtn.disabled = true;
      const payload = Object.assign({}, d, { lines: d.lines.filter((l) => l.trim()) });
      try {
        const saved = existing ? await api('PUT', '/reminders/' + existing.id, payload) : await api('POST', '/reminders', payload);
        const i = S.reminders.findIndex((r) => r.id === saved.id);
        if (i >= 0) S.reminders[i] = saved; else S.reminders.push(saved);
        finish();
        rerender();
        toast(existing ? 'Saved ' + saved.name : 'Created ' + saved.name + (saved.enabled ? '. It is running.' : ''));
        refreshAudit();
      } catch (e) { toast(e.message, 'error'); saveBtn.disabled = false; }
    });

    function localCheck() {
      if (!d.name.trim()) return 'Give the reminder a name.';
      if (!d.channel_id) return 'Pick a channel to post in.';
      if (!d.lines.some((l) => l.trim())) return 'Add at least one message line.';
      if (d.lines.some((l) => l.length > 1800)) return 'A line is longer than 1800 characters.';
      if (d.schedule.kind === 'every' && (d.schedule.minutes < 5 || d.schedule.minutes > 10080)) return 'Post at most every 5 minutes and at least once a week.';
      if (d.schedule.kind === 'daily' && !(d.schedule.times || []).length) return 'Add at least one time of day.';
      if (!!d.active_from !== !!d.active_to) return 'Active hours need both a start and an end.';
      if (d.stop_when_back && !d.user_id) return 'Pick the member to watch for.';
      return null;
    }

    const preview = h('div', null);
    const drawPreview = () => {
      clear(preview);
      const lines = d.lines.filter((l) => l.trim());
      const idx = lines.length ? Math.min(previewIndex, lines.length - 1) : 0;
      const next = nextPost(d);
      const msg = (text, time) => h('div', { class: 'msg' }, h('span', { class: 'brand-mark' }, h('span', null, 'L')),
        h('div', { style: 'min-width:0' }, h('div', { class: 'msg-head' }, h('b', null, 'Loduchand'), h('span', { class: 'msg-bot' }, 'BOT'), h('span', { class: 'msg-time' }, time)),
          h('div', { class: 'msg-text' }, text)));
      const chName = chan(d.channel_id);
      append(preview, [
        h('div', { class: 'preview-tools' }, h('span', null, chName ? '#' + chName.name : 'No channel yet'), h('span', { class: 'grow' }),
          lines.length > 1 ? [
            h('span', null, (d.order === 'random' ? 'Random · ' : 'Line ') + (idx + 1) + ' of ' + lines.length),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Previous line', onclick: () => { previewIndex = (idx - 1 + lines.length) % lines.length; drawPreview(); } }, icon('up')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Next line', onclick: () => { previewIndex = (idx + 1) % lines.length; drawPreview(); } }, icon('down')),
          ] : null),
        h('div', { class: 'preview' },
          lines.length ? msg(fillLine(lines[idx], d, member), next ? (next.day === 'today' ? 'Today at ' : next.day === 'tomorrow' ? 'Tomorrow at ' : '') + next.time : 'Today') : h('p', { style: 'color:#949ba4' }, 'Write a line to see it here.'),
          d.stop_when_back && d.welcome_line.trim() ? msg(fillLine(d.welcome_line, d, member), 'When they’re back') : null),
        h('p', { class: 'next-note' }, icon('clock'), next ? 'Next post around ' + next.time + ' IST ' + next.day + (d.enabled ? '' : ' once switched on') + '.' : 'No post is due in the next week with these settings.'),
      ]);
    };

    const field = (label, control, hint, opt) => h('div', { class: opt && opt.full ? 'full' : '' },
      h('label', { class: 'label', for: control.id || null }, label, opt && opt.optional ? h('span', { class: 'opt' }, ' · optional') : null), control, hint ? h('div', { class: 'hint' }, hint) : null);

    const draw = () => {
      const scroll = body.scrollTop;
      clear(body);

      // basics
      const name = h('input', { class: 'input', id: 'r-name', value: d.name, maxlength: '100', placeholder: 'e.g. Where is Zoya?', autocomplete: 'off' });
      name.addEventListener('input', () => { d.name = name.value; });
      const c = chan(d.channel_id);
      const chBtn = h('button', { class: 'picker-btn', type: 'button', id: 'r-channel', 'aria-haspopup': 'listbox' }, h('span', { class: 'glyph' }, '#'),
        h('span', { class: 'value' + (c ? '' : ' placeholder') }, c ? c.name : 'Choose a channel'), c && c.category ? h('small', { style: 'color:var(--faint)' }, c.category) : null, icon('chevron'));
      chBtn.addEventListener('click', () => openPicker(chBtn, { title: 'Channel', placeholder: 'Search channels', load: channelItems('text', [d.channel_id]), onPick: (it) => { d.channel_id = it.id; draw(); } }));
      const enabled = switchEl(d.enabled, 'Reminder running', (on, btn) => { d.enabled = on; btn.set(on); drawPreview(); });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('bell'), 'Basics', h('span', { class: 'right' }, enabled)),
        h('div', { class: 'form-grid' }, field('Name', name, 'Only admins see this.'), field('Posts in', chBtn))));

      // lines
      const lines = h('div', { class: 'lines' });
      let lastFocused = null;
      d.lines.forEach((line, i) => {
        const ta = h('textarea', { class: 'textarea', rows: '1', 'aria-label': 'Line ' + (i + 1), placeholder: i === 0 ? 'e.g. {mention}, it has been {days} days!' : 'Another line', maxlength: '2000' });
        ta.value = line;
        const counter = h('div', { class: 'line-count' + (line.length > 1800 ? ' over' : ''), 'aria-live': 'polite' }, line.length > 1500 ? line.length + ' / 1800' : '');
        const grow = () => { ta.style.height = 'auto'; ta.style.height = Math.min(220, ta.scrollHeight + 2) + 'px'; };
        ta.addEventListener('input', () => { d.lines[i] = ta.value; grow(); counter.textContent = ta.value.length > 1500 ? ta.value.length + ' / 1800' : ''; counter.classList.toggle('over', ta.value.length > 1800); previewIndex = i; drawPreview(); });
        ta.addEventListener('focus', () => { lastFocused = ta; lastFocused.dataset.i = i; });
        requestAnimationFrame(grow);
        const move = (dir) => { const j = i + dir; if (j < 0 || j >= d.lines.length) return; const t = d.lines[i]; d.lines[i] = d.lines[j]; d.lines[j] = t; draw(); const again = body.querySelectorAll('.line-row textarea')[j]; if (again) again.focus(); };
        lines.appendChild(h('div', { class: 'line-row' }, h('span', { class: 'line-n', 'aria-hidden': 'true' }, i + 1),
          h('div', null, ta, counter),
          h('div', { class: 'line-tools' },
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Move line ' + (i + 1) + ' up', disabled: i === 0, onclick: () => move(-1) }, icon('up')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Move line ' + (i + 1) + ' down', disabled: i === d.lines.length - 1, onclick: () => move(1) }, icon('down')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Remove line ' + (i + 1), disabled: d.lines.length === 1, onclick: () => { d.lines.splice(i, 1); draw(); } }, icon('trash')))));
      });
      const insert = (ph) => {
        const ta = lastFocused && lastFocused.isConnected ? lastFocused : body.querySelector('.line-row textarea');
        if (!ta) return;
        const at = ta.selectionStart || ta.value.length;
        ta.value = ta.value.slice(0, at) + ph + ta.value.slice(ta.selectionEnd || at);
        ta.dispatchEvent(new Event('input'));
        ta.focus();
        ta.selectionStart = ta.selectionEnd = at + ph.length;
      };
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('message'), 'Messages', h('span', { class: 'right' }, segmented([['rotate', 'In turn', 'repeat'], ['random', 'Random', 'shuffle']], d.order, 'Line order', (v) => { d.order = v; drawPreview(); }))),
        lines,
        h('div', { style: 'display:flex;gap:10px;align-items:center;flex-wrap:wrap' },
          h('button', { class: 'btn sm', type: 'button', onclick: () => { d.lines.push(''); draw(); const all = body.querySelectorAll('.line-row textarea'); all[all.length - 1].focus(); } }, icon('plus'), 'Add line'),
          h('div', { class: 'placeholders' }, 'Insert:', ['{name}', '{mention}', '{hours}', '{days}'].map((ph) => h('button', { class: 'ph', type: 'button', onmousedown: (e) => e.preventDefault(), onclick: () => insert(ph), 'data-tip': ph === '{name}' ? 'The display name below' : ph === '{mention}' ? 'Pings the member' : 'Counted from “since”' }, ph))))));

      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('eye'), 'Preview'), preview));

      // schedule
      const sched = h('div', null);
      if (d.schedule.kind === 'every') {
        const factor = everyUnit === 'days' ? 1440 : everyUnit === 'hours' ? 60 : 1;
        const n = h('input', { class: 'input', type: 'number', id: 'r-every', min: '1', step: '1', value: String(Math.round((d.schedule.minutes / factor) * 100) / 100), style: 'max-width:110px' });
        const unit = h('select', { class: 'select', 'aria-label': 'Unit', style: 'max-width:130px' }, [['minutes', 'minutes'], ['hours', 'hours'], ['days', 'days']].map(([v, t]) => h('option', { value: v, selected: v === everyUnit }, t)));
        const err = h('div', { class: 'hint' });
        const sync = () => {
          const f = unit.value === 'days' ? 1440 : unit.value === 'hours' ? 60 : 1;
          everyUnit = unit.value;
          d.schedule.minutes = Math.round((parseFloat(n.value) || 0) * f);
          const bad = d.schedule.minutes < 5 || d.schedule.minutes > 10080;
          n.classList.toggle('invalid', bad);
          err.className = bad ? 'error-text' : 'hint';
          err.textContent = bad ? 'Between 5 minutes and 7 days.' : scheduleWords(d.schedule) + ', lined up on the clock (every hour posts on the hour).';
          drawPreview();
        };
        n.addEventListener('input', sync);
        unit.addEventListener('change', sync);
        append(sched, [h('label', { class: 'label', for: 'r-every' }, 'Every'), h('div', { style: 'display:flex;gap:8px' }, n, unit), err]);
        setTimeout(sync, 0);
      } else {
        const times = h('div', { class: 'times' });
        (d.schedule.times || []).forEach((t) => times.appendChild(h('span', { class: 'chip' }, icon('clock'), h('span', { class: 'chip-text mono' }, t),
          h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + t, onclick: () => { d.schedule.times = d.schedule.times.filter((x) => x !== t); draw(); } }, icon('x')))));
        const tIn = h('input', { class: 'input', type: 'time', 'aria-label': 'Time to add', id: 'r-time' });
        const add = () => { if (parseHm(tIn.value) === null) { tIn.focus(); return; } const v = hm(parseHm(tIn.value)); if (!d.schedule.times.includes(v)) { d.schedule.times.push(v); d.schedule.times.sort(); } draw(); const again = body.querySelector('#r-time'); if (again) again.focus(); };
        tIn.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); add(); } });
        if (!(d.schedule.times || []).length) times.appendChild(h('span', { class: 'hint', style: 'margin:0' }, 'No times yet.'));
        append(sched, [h('label', { class: 'label', for: 'r-time' }, 'Every day at (IST)'), times, h('div', { class: 'time-add', style: 'margin-top:8px' }, tIn, h('button', { class: 'btn sm', type: 'button', onclick: add }, icon('plus'), 'Add time'))]);
      }
      const limit = h('input', { type: 'checkbox', checked: !!d.active_from });
      const from = h('input', { class: 'input', type: 'time', value: d.active_from, 'aria-label': 'Active from' });
      const to = h('input', { class: 'input', type: 'time', value: d.active_to, 'aria-label': 'Active until' });
      limit.addEventListener('change', () => { if (limit.checked) { d.active_from = d.active_from || '10:00'; d.active_to = d.active_to || '23:00'; } else { d.active_from = ''; d.active_to = ''; } draw(); });
      from.addEventListener('input', () => { d.active_from = from.value; drawPreview(); });
      to.addEventListener('input', () => { d.active_to = to.value; drawPreview(); });
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('calendar'), 'Schedule', h('span', { class: 'right' }, segmented([['every', 'Repeat'], ['daily', 'Daily times']], d.schedule.kind, 'Schedule type', (v) => {
          d.schedule = v === 'every' ? { kind: 'every', minutes: 60 } : { kind: 'daily', times: ['10:00'] };
          everyUnit = 'hours';
          draw();
        }))),
        sched,
        h('div', null,
          h('label', { class: 'check' }, limit, h('span', null, 'Only post during these hours', h('small', null, 'India time. Quiet the rest of the day.'))),
          d.active_from ? h('div', { style: 'display:flex;gap:8px;align-items:center;margin:8px 0 0 26px;max-width:340px' }, from, h('span', { style: 'color:var(--faint)' }, 'to'), to) : null)));

      // member
      const mBtn = h('button', { class: 'picker-btn', type: 'button', id: 'r-member', 'aria-haspopup': 'listbox' },
        member ? avatar(member.avatar, member.name, 'xs') : icon('user'), h('span', { class: 'value' + (d.user_id ? '' : ' placeholder') }, member ? member.name : d.user_id ? 'Member ' + d.user_id : 'Nobody in particular'), icon('chevron'));
      mBtn.addEventListener('click', () => openPicker(mBtn, { title: 'Member', placeholder: 'Search members by name', debounce: 180, load: memberItems, onPick: (it) => { d.user_id = it.id; member = it.member; if (!d.user_name) d.user_name = it.member.name; draw(); } }));
      if (d.user_id && !member) memberById(d.user_id).then((m) => { if (m) { member = m; draw(); } });
      const uname = h('input', { class: 'input', id: 'r-uname', value: d.user_name, placeholder: member ? member.name : 'Shown as {name}', maxlength: '100' });
      uname.addEventListener('input', () => { d.user_name = uname.value; drawPreview(); });
      const since = h('input', { class: 'input', type: 'datetime-local', id: 'r-since', value: toIstInput(d.since) });
      since.addEventListener('input', () => { d.since = fromIstInput(since.value); drawPreview(); });
      const back = h('input', { type: 'checkbox', checked: d.stop_when_back, disabled: !d.user_id });
      back.addEventListener('change', () => { d.stop_when_back = back.checked; draw(); });
      const welcome = h('input', { class: 'input', id: 'r-welcome', value: d.welcome_line, placeholder: 'e.g. {mention} is back!', maxlength: '1800' });
      welcome.addEventListener('input', () => { d.welcome_line = welcome.value; drawPreview(); });
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('user'), 'About a member', h('span', { class: 'right', style: 'color:var(--faint);font-size:12.5px' }, 'optional')),
        h('div', { class: 'form-grid' },
          field('Member', h('div', { class: 'picker-row', id: 'r-member-row' }, mBtn, d.user_id ? h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Clear member', onclick: () => { d.user_id = ''; member = null; d.stop_when_back = false; draw(); } }, icon('x')) : null), 'Used for {mention}.'),
          field('Display name', uname, 'Used for {name}.'),
          field('Since (IST)', since, '{hours} and {days} count from here.', { full: false }),
          h('div', null)),
        h('label', { class: 'check' }, back, h('span', null, 'Stop when they’re back in the server', h('small', null, d.user_id ? 'Switches the reminder off and posts the welcome line once.' : 'Pick a member first.'))),
        d.stop_when_back ? field('Welcome line', welcome) : null));

      // ends
      const ends = h('input', { class: 'input', type: 'datetime-local', id: 'r-ends', value: toIstInput(d.ends), style: 'max-width:260px' });
      ends.addEventListener('input', () => { d.ends = fromIstInput(ends.value); drawPreview(); });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('clock'), 'Ends', h('span', { class: 'right', style: 'color:var(--faint);font-size:12.5px' }, 'optional')),
        h('div', { style: 'display:flex;gap:8px;align-items:center;flex-wrap:wrap' }, ends, d.ends ? h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { d.ends = ''; draw(); } }, 'Never end') : h('span', { class: 'hint', style: 'margin:0' }, 'Runs until switched off.'))));

      drawPreview();
      body.scrollTop = scroll;
    };
    draw();
    requestAnimationFrame(() => { const n = body.querySelector('#r-name'); if (n) n.focus({ preventScroll: true }); });
  }

  // --- restart --------------------------------------------------------------------

  async function restartFlow() {
    const ok = await confirmDialog({
      title: 'Restart Loduchand?', icon: 'alert', danger: true, confirm: 'Restart now',
      body: h('div', { class: 'restart-body' },
        h('p', null, 'The bot goes offline for about 15 seconds and comes back on its own.'),
        h('ul', { class: 'warn-list' },
          h('li', null, icon('alert'), 'Fights and battle royales in progress are cut off mid-fight.'),
          h('li', null, icon('alert'), 'A running quiz loses the question on screen.'),
          h('li', null, icon('alert'), 'Commands used during the restart get no answer.'))),
    });
    if (!ok) return;
    const before = S.status ? S.status.bot.started : 0;
    try { await api('POST', '/restart'); } catch (e) { toast(e.message, 'error'); return; }
    const text = h('p', null, 'Waiting for the bot to come back…');
    const overlay = h('div', { class: 'restarting', role: 'alertdialog', 'aria-live': 'assertive', 'aria-label': 'Restarting' },
      h('div', { class: 'modal' }, h('span', { class: 'spinner' }), h('h2', null, 'Restarting Loduchand'), text));
    document.body.appendChild(overlay);
    const started = Date.now();
    await new Promise((r) => setTimeout(r, 4000));
    for (;;) {
      try {
        const res = await fetch('/api/status', { credentials: 'same-origin', headers: { Accept: 'application/json' } });
        if (res.ok) {
          const st = await res.json();
          if (st.bot.started !== before) {
            S.status = st; S.statusAt = Date.now(); S.restartPending = false;
            overlay.remove();
            await loadAll().catch(() => {});
            renderBanner(); rerender();
            toast('Loduchand is back online');
            return;
          }
        }
      } catch (_) { /* still down */ }
      const waited = Math.round((Date.now() - started) / 1000);
      if (waited > 90) {
        clear(overlay.firstChild);
        append(overlay.firstChild, [icon('alert'), h('h2', null, 'Still not back'), h('p', null, 'It has been ' + waited + ' seconds. Check the server, or keep waiting.'),
          h('button', { class: 'btn', type: 'button', onclick: () => location.reload() }, 'Reload the page')]);
        return;
      }
      text.textContent = 'Waiting for the bot to come back… ' + waited + 's';
      await new Promise((r) => setTimeout(r, 2500));
    }
  }

  // --- boot -----------------------------------------------------------------------

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && layers.length) {
      const top = layers[layers.length - 1];
      (top.attempt || top.close)();
      e.preventDefault();
      return;
    }
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test((e.target && e.target.tagName) || '') || (e.target && e.target.isContentEditable);
    if (e.key === '/' && !typing && !e.metaKey && !e.ctrlKey && S.booted && shell && !layers.length) {
      e.preventDefault();
      shell.search.focus();
    }
    if (e.key === 'Escape' && shell && shell.app.classList.contains('nav-open')) toggleNav(false);
  });

  window.addEventListener('beforeunload', (e) => { if (S.booted && dirtyCount()) { e.preventDefault(); e.returnValue = ''; } });
  window.addEventListener('hashchange', onHashChange);

  async function boot() {
    const m = /^#t=([A-Za-z0-9_-]{10,128})$/.exec(location.hash);
    const keep = location.search;
    if (m) {
      // Take the token out of the address bar and history before anything else.
      history.replaceState(null, '', '/' + keep);
      renderSignIn({ busy: true });
      try {
        await api('POST', '/login', { token: m[1] });
      } catch (e) {
        renderSignIn({ error: e.message });
        return;
      }
    } else if (location.pathname === '/login') {
      history.replaceState(null, '', '/' + keep + (location.hash && !location.hash.startsWith('#t=') ? location.hash : ''));
      if (location.hash.startsWith('#t=')) { history.replaceState(null, '', '/' + keep); renderSignIn({ error: 'That sign-in link looks broken. Run /panel in Discord for a new one.' }); return; }
    }
    try {
      await loadAll();
    } catch (e) {
      if (e.status === 401) renderSignIn({});
      else if (e.status === 403) renderSignIn({ error: e.message });
      else renderSignIn({ error: e.message || 'The panel could not load.' });
      return;
    }
    S.booted = true;
    currentHash = location.hash;
    renderShell();
    rerender();
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
