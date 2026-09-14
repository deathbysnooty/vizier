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
    left: '<path d="m15 6-6 6 6 6"/>',
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
    reply: '<path d="M9 14 4 9l5-5"/><path d="M4 9h10a6 6 0 0 1 6 6v5"/>',
    zap: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>',
    trophy: '<path d="M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0z"/><path d="M17 5h3v2a3 3 0 0 1-3 3M7 5H4v2a3 3 0 0 0 3 3"/>',
    bot: '<rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 4v4M9 13h.01M15 13h.01M9 17h6"/>',
    smile: '<circle cx="12" cy="12" r="9"/><path d="M8.5 14.5a4.5 4.5 0 0 0 7 0M9 9.5h.01M15 9.5h.01"/>',
    target: '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="5"/><circle cx="12" cy="12" r="1"/>',
    flask: '<path d="M9 3h6M10 3v6L4.5 18.5A1.7 1.7 0 0 0 6 21h12a1.7 1.7 0 0 0 1.5-2.5L14 9V3"/><path d="M7 15h10"/>',
    table: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 10h18M3 15h18M9 4v16"/>',
    chart: '<path d="M4 20V4M4 20h16"/><rect x="7" y="12" width="3" height="5"/><rect x="12" y="8" width="3" height="9"/><rect x="17" y="5" width="3" height="12"/>',
    pause: '<rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/>',
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
      const modal = h('div', { class: 'modal' + (opts.wide ? ' wide' : ''), role: 'alertdialog', 'aria-modal': 'true', 'aria-labelledby': titleId },
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
    rules: [],
    emojis: null,
    guard: null, // unsaved-change count for pages outside the settings form
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
    S.rules = await api('GET', '/autoreplies').catch(() => []);
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
    nav.appendChild(navItem('#/houses', icon('trophy'), 'House Cup', h('span', { class: 'nav-live', 'aria-label': 'live' })));

    const pinned = prefs.pins.map(sectionById).filter(Boolean);
    if (pinned.length) nav.appendChild(h('div', { class: 'nav-group' }, h('span', { class: 'nav-label' }, 'Pinned'), pinned.map(sectionItem)));
    const active = S.reminders.filter((r) => r.enabled).length;
    const activeRules = S.rules.filter((r) => r.enabled).length;
    const count = (on, all) => (all ? h('span', { class: 'nav-count', 'aria-label': on + ' of ' + all + ' on' }, on + '/' + all) : null);
    nav.appendChild(h('div', { class: 'nav-group' }, h('span', { class: 'nav-label' }, 'Manage'),
      navItem('#/reminders', icon('bell'), 'Reminders', count(active, S.reminders.length)),
      navItem('#/autoreplies', icon('reply'), 'Auto-responses', count(activeRules, S.rules.length)),
      navItem('#/members', icon('users'), 'Members'),
      navItem('#/agent', icon('bot'), 'Bot behaviour'),
      navItem('#/activity', icon('activity'), 'Activity log'),
      navItem('#/commands', icon('slash'), 'Commands')));
    const rest = S.sections.filter((s) => !prefs.pins.includes(s.id));
    if (rest.length) nav.appendChild(h('div', { class: 'nav-group' }, h('span', { class: 'nav-label' }, 'Features'), rest.map(sectionItem)));
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
      ['Auto-responses', '#/autoreplies', 'reply', 'Answer or react to set words'],
      ['House Cup', '#/houses', 'trophy', 'Live house points, top scorers, latest points'],
      ['Bot behaviour', '#/agent', 'bot', 'Personality, tone, chattiness, model'],
      ['Members', '#/members', 'users', 'Profiles, what the bot sees, mods’ notes'],
      ['Scorers today', '#/houses/scorers', 'zap', 'Today’s points and daily limits'],
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
    S.rules.forEach((r) => items.push({ kind: 'Auto-responses', title: r.name, sub: r.triggers.join(', '), href: '#/autoreplies/' + r.id, lead: icon('reply'), hay: r.name + ' ' + r.triggers.join(' ') + ' ' + r.replies.join(' ') }));
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
      const order = ['Pages', 'Features', 'Settings', 'Commands', 'Reminders', 'Auto-responses'];
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

  function dirtyCount() { return S.fields.filter((f) => f.dirty()).length + (S.guard ? S.guard() : 0); }

  async function onHashChange() {
    if (!S.booted) return;
    const target = location.hash;
    const leavingSection = target.split('?')[0] !== currentHash.split('?')[0];
    if (!skipGuard && leavingSection && dirtyCount()) {
      history.replaceState(null, '', currentHash);
      const ok = await confirmDialog({ title: 'Leave without saving?', icon: 'alert', body: plural(dirtyCount(), 'unsaved change') + ' on this page will be lost.', confirm: 'Discard changes', danger: true });
      if (!ok) return;
      if (S.discard) S.discard();
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
    S.guard = null;
    S.discard = null;
    if (agentBar) { agentBar.remove(); agentBar = null; }
    renderSidebar();
    const page = clear(shell.page);
    removeSavebar();
    switch (r.name) {
      case 'section': renderSection(page, r.parts[1], r.q.get('k')); break;
      case 'reminders': renderReminders(page); if (r.parts[1]) openReminderEditor(r.parts[1]); break;
      case 'autoreplies': renderRules(page); if (r.parts[1]) openRuleEditor(r.parts[1]); break;
      case 'houses': if (r.parts[1] === 'scorers') renderScorers(page); else renderHouses(page); break;
      case 'members': if (r.parts[1]) renderProfile(page, r.parts[1], r.parts[2]); else renderMembers(page); break;
      case 'agent': renderAgent(page); break;
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
      [h('a', { class: 'btn', href: '#/houses/scorers' }, icon('zap'), 'Scorers today'),
        h('button', { class: 'btn', type: 'button', onclick: restartFlow }, icon('restart'), 'Restart bot')]));

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
    if (!e.kind) {
      diff.appendChild(h('span', { class: 'badge' }, e.change || 'Changed'));
    } else if (e.section) {
      if (e.change === 'Reset to .env') append(diff, [h('span', { class: 'badge src-env' }, icon('reset'), 'Back to .env')]);
      else append(diff, [oldValue(e), h('span', { class: 'arrow', 'aria-label': 'to' }, '→'), formatValue(e.new, e.kind)]);
    }
    return h('li', { class: 'change' }, avatar(e.user_avatar, who),
      h('div', { style: 'min-width:0' }, h('div', { class: 'change-line' }, h('b', null, who), ' ', e.kind ? 'changed ' : 'updated ', h('b', null, e.label), e.section ? h('span', { style: 'color:var(--faint)' }, ' · ' + e.section.title) : null), diff),
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
    if (sec.id === 'autoreplies') {
      page.appendChild(h('a', { class: 'banner inline info link-banner', href: '#/autoreplies' }, icon('reply'),
        h('p', null, h('b', null, 'The rules live on the Auto-responses page. '), h('span', null, plural(S.rules.length, 'rule') + ', ' + S.rules.filter((r) => r.enabled).length + ' switched on.')), icon('right')));
    }
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
      if (!sectionById('autoreplies')) secSel.appendChild(h('option', { value: 'autoreplies', selected: section === 'autoreplies' }, '💬  Auto-responses'));
      secSel.appendChild(h('option', { value: 'agent', selected: section === 'agent' }, '🤖  Bot behaviour'));
      if (!sectionById('members')) secSel.appendChild(h('option', { value: 'members', selected: section === 'members' }, '👤  Members'));
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
        if (!e.kind) diff = h('span', { class: 'badge' }, e.change || 'Changed');
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

  // --- drawers ------------------------------------------------------------------------

  /** A side panel editor. `isDirty()` guards closing; returns { body, finish, saveBtn }. */
  function openDrawer(opts) {
    const before = document.activeElement;
    const body = h('div', { class: 'drawer-body' });
    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, opts.saveLabel || 'Save');
    const titleId = 'drawer-title';
    const drawer = h('div', { class: 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': titleId },
      h('div', { class: 'drawer-head' }, h('div', { class: 'grow' }, h('h2', { id: titleId }, opts.title), opts.sub ? h('div', { class: 'sub' }, opts.sub) : null),
        h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Close', onclick: () => attempt() }, icon('x'))),
      body,
      h('div', { class: 'drawer-foot' },
        opts.onDelete ? h('button', { class: 'btn danger', type: 'button', onclick: opts.onDelete }, icon('trash'), 'Delete') : null,
        h('span', { class: 'grow' }),
        h('button', { class: 'btn ghost', type: 'button', onclick: () => attempt() }, 'Cancel'),
        saveBtn));
    const scrim = h('div', { class: 'scrim', onclick: () => attempt() });
    const finish = (silent) => {
      drawer.remove(); scrim.remove(); popLayer(layer);
      if (!silent && opts.returnHash && location.hash !== opts.returnHash) { history.replaceState(null, '', opts.returnHash); currentHash = opts.returnHash; }
      if (before && before.isConnected && before.focus) before.focus();
    };
    const attempt = async () => {
      if (opts.isDirty && opts.isDirty()) {
        const ok = await confirmDialog({ title: 'Discard your changes?', icon: 'alert', body: 'What you changed here has not been saved.', confirm: 'Discard', danger: true });
        if (!ok) return;
      }
      finish();
    };
    const layer = pushLayer({ drawer: true, close: () => finish(true), attempt });
    drawer.addEventListener('keydown', (e) => { if (e.key === 'Tab' && !layers.some((l) => l.popover)) trapFocus(drawer, e); });
    saveBtn.addEventListener('click', () => opts.onSave(saveBtn));
    $('#layers').appendChild(scrim);
    $('#layers').appendChild(drawer);
    return { body, finish, saveBtn };
  }

  function discordMsg(author, avatarEl, time, text, extra) {
    return h('div', { class: 'msg' }, avatarEl,
      h('div', { style: 'min-width:0' }, extra && extra.replyTo ? h('div', { class: 'msg-ref' }, icon('reply'), h('span', { class: 'mention' }, '@' + extra.replyTo), ' ', h('span', { class: 'msg-ref-text' }, extra.replyText || '')) : null,
        h('div', { class: 'msg-head' }, h('b', null, author), extra && extra.bot ? h('span', { class: 'msg-bot' }, 'BOT') : null, h('span', { class: 'msg-time' }, time)),
        h('div', { class: 'msg-text' }, text),
        extra && extra.reactions && extra.reactions.length ? h('div', { class: 'msg-reactions' }, extra.reactions.map((r) => h('span', { class: 'msg-reaction' }, emojiEl(r), h('b', null, '1')))) : null));
  }

  // --- auto-responses ------------------------------------------------------------------

  const MATCH_MODES = [
    ['whole_word', 'Whole word or phrase', 'The trigger on its own, not inside a longer word.', '“hi” matches “hi there”, not “this”'],
    ['contains', 'Anywhere', 'Anywhere in the message, even inside a longer word.', '“hi” matches “this”'],
    ['exact', 'The whole message', 'The message is the trigger and nothing else.', '“gm” matches “gm”, not “gm all”'],
    ['starts_with', 'Starts with', 'The message begins with the trigger.', '“bhai” matches “bhai sun”, not “sun bhai”'],
    ['pattern', 'Pattern', 'A regular expression, for advanced rules.', '^when.*quiz matches “when is quiz?”'],
  ];
  const matchMode = (m) => MATCH_MODES.find((x) => x[0] === m) || MATCH_MODES[0];

  function chanceWords(n) {
    if (n >= 100) return 'Every time';
    if (n >= 75) return 'Most of the time';
    if (n >= 40) return 'About ' + n + '% of the time';
    if (n >= 15) return 'Now and then (' + n + '%)';
    return 'Rarely (' + n + '%)';
  }
  function cooldownWords(secs) {
    if (!secs) return 'no cooldown';
    if (secs % 3600 === 0) return plural(secs / 3600, 'hour') + ' cooldown';
    if (secs % 60 === 0) return secs / 60 + ' min cooldown';
    if (secs < 60) return secs + ' sec cooldown';
    return Math.floor(secs / 60) + ' min ' + (secs % 60) + ' sec cooldown';
  }

  async function loadEmojis() {
    if (S.emojis) return S.emojis;
    try { S.emojis = await api('GET', '/discord/emojis'); } catch (_) { S.emojis = []; }
    return S.emojis;
  }

  /** A reaction as written in a rule: unicode, or a custom `<:name:id>` / `name:id`. */
  function emojiEl(text) {
    const m = /^<?(a)?:?([A-Za-z0-9_~]+):(\d{5,20})>?$/.exec(String(text).trim());
    if (!m) return h('span', { class: 'emoji-text' }, text);
    const known = (S.emojis || []).find((e) => e.id === m[3]);
    const url = known ? known.url : 'https://cdn.discordapp.com/emojis/' + m[3] + (m[1] ? '.gif' : '.png') + '?size=48';
    const img = h('img', { class: 'emoji-img', src: url, alt: ':' + m[2] + ':', title: ':' + m[2] + ':', referrerpolicy: 'no-referrer' });
    img.addEventListener('error', () => img.replaceWith(h('span', { class: 'emoji-text' }, ':' + m[2] + ':')), { once: true });
    return img;
  }

  function channelList(ids) {
    return ids.map((id, i) => [i ? ', ' : '', channelRef(id)]);
  }

  function masterSwitchBanner() {
    const sec = sectionById('autoreplies');
    const setting = sec && sec.settings.find((x) => x.key === 'VIZIER_AUTOREPLIES');
    if (!setting) return null;
    const on = isOn(baseline(setting));
    const el = h('div', { class: 'master ' + (on ? 'is-on' : 'is-off') });
    const sw = switchEl(on, 'All auto-responses', async (next, btn) => {
      if (!next) {
        const ok = await confirmDialog({ title: 'Pause every auto-response?', icon: 'alert', body: 'No rule fires until this is switched back on. Nothing is deleted.', confirm: 'Pause all', danger: true });
        if (!ok) return;
      }
      btn.disabled = true;
      try {
        const updated = await api('PUT', '/settings/VIZIER_AUTOREPLIES', { value: next ? 'on' : 'off' });
        replaceSetting(updated);
        toast(next ? 'Auto-responses are on' : 'Auto-responses are paused');
        refreshStatus(); refreshAudit();
        rerender();
      } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
    });
    append(el, [
      h('span', { class: 'master-icon', 'aria-hidden': 'true' }, on ? icon('zap') : icon('pause')),
      h('div', { class: 'grow' }, h('b', null, on ? 'Auto-responses are on' : 'Auto-responses are paused'),
        h('p', null, on ? 'Rules that are switched on answer and react as set below.' : 'The master switch is off, so no rule fires, whatever its own switch says.')),
      sw,
    ]);
    return el;
  }

  function renderRules(page) {
    document.title = 'Auto-responses · Loduchand';
    page.appendChild(pageHead('Auto-responses', 'Let the bot answer or react when people say certain words. Each rule has its own switch.',
      h('a', { class: 'btn primary', href: '#/autoreplies/new' }, icon('plus'), 'New rule')));
    const master = masterSwitchBanner();
    if (master) page.appendChild(master);
    const masterOn = !master || master.classList.contains('is-on');
    const input = h('input', { type: 'search', placeholder: 'Filter rules by name, trigger or reply', 'aria-label': 'Filter rules' });
    const count = h('span', { class: 'count', 'aria-live': 'polite' });
    const grid = h('div', { class: 'reminders rules' });
    const draw = () => {
      clear(grid);
      const q = input.value.trim().toLowerCase();
      const shown = S.rules.filter((r) => !q || (r.name + ' ' + r.triggers.join(' ') + ' ' + r.replies.join(' ')).toLowerCase().includes(q));
      shown.forEach((r) => grid.appendChild(ruleCard(r, masterOn)));
      count.textContent = shown.length === S.rules.length ? plural(S.rules.length, 'rule') : shown.length + ' of ' + S.rules.length;
      if (!q) grid.appendChild(h('a', { class: 'new-card', href: '#/autoreplies/new' }, icon('plus'), S.rules.length ? 'New rule' : 'Make your first rule'));
      else if (!shown.length) grid.appendChild(h('div', { class: 'card empty', style: 'grid-column:1/-1' }, h('p', null, 'No rule matches “' + q + '”.')));
    };
    input.addEventListener('input', draw);
    if (S.rules.length > 3) page.appendChild(h('div', { class: 'toolbar' }, h('label', { class: 'search-box' }, icon('search'), input), count));
    page.appendChild(grid);
    draw();
    if (!S.emojis) loadEmojis().then(() => { if (grid.isConnected) draw(); });
  }

  function ruleCard(r, masterOn) {
    const el = h('article', { class: 'reminder rule' + (r.enabled ? '' : ' is-off') + (masterOn ? '' : ' master-off'), 'aria-label': r.name });
    const sw = switchEl(r.enabled, (r.enabled ? 'Switch off ' : 'Switch on ') + r.name, async (on, btn) => {
      btn.disabled = true;
      try {
        const updated = await api('POST', '/autoreplies/' + r.id + '/toggle');
        Object.assign(r, updated);
        el.replaceWith(ruleCard(r, masterOn));
        renderSidebar();
        toast(r.name + (updated.enabled ? ' is on' : ' is off'));
        refreshAudit();
      } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
    }, { noText: true });
    const mode = matchMode(r.match_mode);
    const triggers = r.triggers.slice(0, 6).map((t) => h('span', { class: 'trigger' + (r.match_mode === 'pattern' ? ' mono' : '') }, t));
    if (r.triggers.length > 6) triggers.push(h('span', { class: 'trigger more' }, '+' + (r.triggers.length - 6)));
    const replies = r.replies.length;
    const facts = h('ul', { class: 'reminder-facts' },
      h('li', null, icon('target'), h('span', null, mode[1], r.case_sensitive ? h('span', { style: 'color:var(--faint)' }, ' · case-sensitive') : null)),
      h('li', null, icon('hash'), h('span', null, r.channels.length ? channelList(r.channels) : 'All channels', r.exclude_channels.length ? h('span', { style: 'color:var(--faint)' }, [' · not ', channelList(r.exclude_channels)]) : null)),
      h('li', null, icon('message'), h('span', null, replies ? plural(replies, 'reply', 'replies') + (replies > 1 ? ', one at random' : '') + (r.as_reply ? ' · as a reply' : ' · in the channel') : 'No reply, reactions only')),
      r.reactions.length ? h('li', null, icon('smile'), h('span', { class: 'reaction-row' }, r.reactions.map(emojiEl))) : null,
      h('li', null, icon('clock'), h('span', null, chanceWords(r.chance) + ' · ' + cooldownWords(r.cooldown_secs))));
    el.appendChild(h('div', { class: 'reminder-head' },
      h('div', { class: 'grow' }, h('h3', null, r.name),
        h('div', { class: 'sub' }, r.enabled ? (masterOn ? h('span', { class: 'badge on' }, h('span', { class: 'dot' }), 'On') : h('span', { class: 'badge paused', 'data-tip': 'The master switch is off' }, icon('pause'), 'Paused by master')) : h('span', { class: 'badge paused' }, 'Off'))),
      sw));
    el.appendChild(h('div', { class: 'triggers', 'aria-label': 'Triggers' }, triggers));
    el.appendChild(facts);
    el.appendChild(h('div', { class: 'reminder-foot' },
      h('span', { class: 'grow' }, r.hits ? 'Fired ' + plural(r.hits, 'time') + (r.last_hit ? ' · last ' + ago(r.last_hit) : '') : 'Not fired yet'),
      h('a', { class: 'btn sm', href: '#/autoreplies/' + r.id }, icon('edit'), 'Edit')));
    return el;
  }

  function blankRule() {
    return { id: 0, name: '', enabled: true, triggers: [], match_mode: 'whole_word', case_sensitive: false, channels: [], exclude_channels: [],
      replies: [''], as_reply: true, reactions: [], chance: 100, cooldown_secs: 30, hits: 0, last_hit: 0 };
  }

  function openRuleEditor(which) {
    const existing = which === 'new' ? null : S.rules.find((r) => String(r.id) === String(which));
    if (which !== 'new' && !existing) { toast('That rule no longer exists', 'error'); history.replaceState(null, '', '#/autoreplies'); currentHash = '#/autoreplies'; return; }
    const d = JSON.parse(JSON.stringify(existing || blankRule()));
    if (!d.replies.length) d.replies = [''];
    const original = JSON.stringify(d);
    let previewIndex = 0;
    let sample = '';
    let sampleChannel = '';
    let cooldownUnit = d.cooldown_secs && d.cooldown_secs % 3600 === 0 ? 'hours' : d.cooldown_secs && d.cooldown_secs % 60 === 0 ? 'minutes' : 'seconds';
    loadEmojis().then(() => draw());

    const payload = () => Object.assign({}, d, { replies: d.replies.filter((x) => x.trim()) });
    const localCheck = () => {
      if (!d.name.trim()) return 'Give the rule a name.';
      if (!d.triggers.length) return 'Add at least one trigger.';
      if (!d.replies.some((x) => x.trim()) && !d.reactions.length) return 'Add a reply or a reaction, or the rule does nothing.';
      return null;
    };
    const ui = openDrawer({
      title: existing ? 'Edit auto-response' : 'New auto-response',
      sub: existing ? (existing.hits ? 'Fired ' + plural(existing.hits, 'time') + (existing.last_hit ? ', last ' + ago(existing.last_hit) : '') : 'Not fired yet') : 'Works as soon as it is saved and switched on',
      saveLabel: existing ? 'Save changes' : 'Create rule',
      returnHash: '#/autoreplies',
      isDirty: () => JSON.stringify(d) !== original,
      onDelete: existing ? async () => {
        const ok = await confirmDialog({ title: 'Delete “' + existing.name + '”?', icon: 'trash', danger: true, body: 'The rule stops and is gone for good. To stop it for a while, switch it off instead.', confirm: 'Delete rule' });
        if (!ok) return;
        try {
          await api('DELETE', '/autoreplies/' + existing.id);
          S.rules = S.rules.filter((r) => r.id !== existing.id);
          ui.finish(); rerender(); toast('Deleted ' + existing.name); refreshAudit();
        } catch (e) { toast(e.message, 'error'); }
      } : null,
      onSave: async (btn) => {
        const problem = localCheck();
        if (problem) { toast(problem, 'error'); return; }
        btn.disabled = true;
        try {
          const saved = existing ? await api('PUT', '/autoreplies/' + existing.id, payload()) : await api('POST', '/autoreplies', payload());
          const i = S.rules.findIndex((r) => r.id === saved.id);
          if (i >= 0) S.rules[i] = saved; else S.rules.push(saved);
          ui.finish(); rerender();
          toast(existing ? 'Saved ' + saved.name : 'Created ' + saved.name);
          refreshAudit();
        } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
      },
    });
    const body = ui.body;

    const preview = h('div', null);
    const result = h('div', { class: 'tester-result', 'aria-live': 'polite' });
    let testTimer = null, testSeq = 0;
    const runTest = () => {
      clearTimeout(testTimer);
      testTimer = setTimeout(async () => {
        const mine = ++testSeq;
        if (!sample.trim()) { clear(result).className = 'tester-result idle'; append(result, [icon('flask'), h('span', null, 'Type a message to see whether this rule would fire.')]); return; }
        try {
          const r = await api('POST', '/autoreplies/test', { rule: payload(), text: sample, channel_id: sampleChannel || null });
          if (mine !== testSeq) return;
          clear(result).className = 'tester-result ' + (r.fires ? 'yes' : r.matches ? 'maybe' : 'no');
          append(result, [icon(r.fires ? 'check' : r.matches ? 'pause' : 'x'), h('span', null, h('b', null, r.fires ? 'Would fire. ' : 'Wouldn’t fire. '), r.why,
            r.fires && (d.chance < 100 || d.cooldown_secs) ? h('small', null, ' Chance and cooldown still apply.') : null)]);
        } catch (e) { if (mine === testSeq) { clear(result).className = 'tester-result no'; append(result, [icon('alert'), h('span', null, e.message)]); } }
      }, 250);
    };
    const drawPreview = () => {
      clear(preview);
      const replies = d.replies.filter((x) => x.trim());
      const idx = replies.length ? Math.min(previewIndex, replies.length - 1) : 0;
      const userText = sample.trim() || d.triggers[0] || 'gm';
      const ch = chan(sampleChannel) || chan(d.channels[0]) || S.channels.find((c) => c.kind === 'text');
      const fill = (t) => t.split(/(\{user\}|\{name\})/g).map((part, i) => (i % 2 ? (part === '{user}' ? h('span', { class: 'mention' }, '@Rohan') : 'Rohan') : part));
      append(preview, [
        h('div', { class: 'preview-tools' }, h('span', null, ch ? '#' + ch.name : ''), h('span', { class: 'grow' }),
          replies.length > 1 ? [h('span', null, 'Reply ' + (idx + 1) + ' of ' + replies.length + ' · picked at random'),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Previous reply', onclick: () => { previewIndex = (idx - 1 + replies.length) % replies.length; drawPreview(); } }, icon('up')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Next reply', onclick: () => { previewIndex = (idx + 1) % replies.length; drawPreview(); } }, icon('down'))] : null),
        h('div', { class: 'preview' },
          discordMsg('Rohan', avatar(null, 'Rohan'), 'Today at 21:04', userText, { reactions: d.reactions }),
          replies.length ? discordMsg('Loduchand', h('span', { class: 'brand-mark' }, h('span', null, 'L')), 'Today at 21:04', fill(replies[idx]),
            { bot: true, replyTo: d.as_reply ? 'Rohan' : null, replyText: userText }) : null),
      ]);
    };

    const field = (label, control, hint) => h('div', null, h('label', { class: 'label', for: control.id || null }, label), control, hint ? h('div', { class: 'hint' }, hint) : null);

    const chipsInput = (list, opts) => {
      const box = h('div', { class: 'chip-input' + (opts.mono ? ' mono' : '') });
      const input = h('input', { type: 'text', id: opts.id, placeholder: list.length ? opts.more : opts.placeholder, 'aria-label': opts.label, autocomplete: 'off', spellcheck: 'false' });
      const add = () => {
        const parts = opts.split ? input.value.split(',') : [input.value];
        let added = false;
        parts.map((x) => x.trim()).filter(Boolean).forEach((x) => { if (!list.includes(x)) { list.push(x); added = true; } });
        input.value = '';
        if (added) { opts.onChange(); const again = body.querySelector('#' + opts.id); if (again) again.focus(); }
      };
      list.forEach((t, i) => box.appendChild(h('span', { class: 'chip' }, opts.render ? opts.render(t) : h('span', { class: 'chip-text' }, t),
        h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + t, onclick: () => { list.splice(i, 1); opts.onChange(); } }, icon('x')))));
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || (opts.split && e.key === ',')) { e.preventDefault(); add(); }
        else if (e.key === 'Backspace' && !input.value && list.length) { list.pop(); opts.onChange(); const again = body.querySelector('#' + opts.id); if (again) again.focus(); }
      });
      input.addEventListener('blur', () => { if (input.value.trim()) add(); });
      box.appendChild(input);
      box.addEventListener('click', (e) => { if (e.target === box) input.focus(); });
      return box;
    };

    const channelChips = (list, label, empty) => {
      const wrap = h('div', { class: 'chips' });
      list.forEach((id, i) => {
        const c = chan(id);
        wrap.appendChild(h('span', { class: 'chip' + (c ? '' : ' missing') }, h('span', { class: 'glyph' }, '#'), h('span', { class: 'chip-text' }, c ? c.name : 'unknown ' + id),
          h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + (c ? c.name : id), onclick: () => { list.splice(i, 1); draw(); } }, icon('x'))));
      });
      if (!list.length) wrap.appendChild(h('span', { class: 'chip ghost-chip' }, empty));
      const add = h('button', { class: 'btn sm', type: 'button' }, icon('plus'), label);
      add.addEventListener('click', () => openPicker(add, { title: label, placeholder: 'Search channels', load: channelItems('text', list), onPick: (it) => {
        if (list.includes(it.id)) return;
        const other = list === d.channels ? d.exclude_channels : d.channels;
        const j = other.indexOf(it.id);
        if (j >= 0) other.splice(j, 1);
        list.push(it.id); draw();
      } }));
      wrap.appendChild(add);
      return wrap;
    };

    function changed() { drawPreview(); runTest(); }

    function draw() {
      const scroll = body.scrollTop;
      const focusId = document.activeElement && body.contains(document.activeElement) ? document.activeElement.id : null;
      clear(body);

      const name = h('input', { class: 'input', id: 'a-name', value: d.name, maxlength: '100', placeholder: 'e.g. Good morning', autocomplete: 'off' });
      name.addEventListener('input', () => { d.name = name.value; });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('reply'), 'Basics', h('span', { class: 'right' }, switchEl(d.enabled, 'Rule on', (on, b) => { b.set(on); d.enabled = on; runTest(); }))),
        field('Name', name, 'Only admins see this.')));

      // triggers and matching
      const mode = matchMode(d.match_mode);
      const modes = h('div', { class: 'modes', role: 'radiogroup', 'aria-label': 'How to match' });
      MATCH_MODES.forEach(([value, title, desc, example]) => {
        modes.appendChild(h('button', { type: 'button', class: 'mode', role: 'radio', 'aria-checked': d.match_mode === value ? 'true' : 'false',
          onclick: () => { d.match_mode = value; draw(); } },
          h('span', { class: 'mode-dot', 'aria-hidden': 'true' }), h('span', null, h('b', null, title), h('small', null, desc), h('code', null, example))));
      });
      const caseBox = h('input', { type: 'checkbox', checked: d.case_sensitive });
      caseBox.addEventListener('change', () => { d.case_sensitive = caseBox.checked; changed(); });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('target'), 'Triggers'),
        h('div', null, h('label', { class: 'label', for: 'a-trigger' }, d.match_mode === 'pattern' ? 'Patterns' : 'Words or phrases'),
          chipsInput(d.triggers, { id: 'a-trigger', label: 'Add a trigger', placeholder: d.match_mode === 'pattern' ? 'Type a pattern, then Enter' : 'Type a word or phrase, then Enter', more: 'Add another…', split: d.match_mode !== 'pattern', mono: d.match_mode === 'pattern', onChange: draw }),
          h('div', { class: 'hint' }, d.match_mode === 'pattern' ? 'Press Enter after each pattern.' : 'Press Enter or a comma after each. Any one of them sets the rule off.')),
        h('div', null, h('span', { class: 'label' }, 'How to match'), modes),
        h('label', { class: 'check' }, caseBox, h('span', null, 'Case-sensitive', h('small', null, d.case_sensitive ? '“GM” and “gm” count as different.' : '“GM”, “Gm” and “gm” all match.')))));

      // where
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('hash'), 'Where'),
        h('div', null, h('span', { class: 'label' }, 'Only in these channels'), channelChips(d.channels, 'Add channel', 'All channels the bot can read'), h('div', { class: 'hint' }, 'Threads count as their parent channel.')),
        h('div', null, h('span', { class: 'label' }, 'Never in'), channelChips(d.exclude_channels, 'Exclude channel', 'No exceptions'))));

      // response
      const replies = h('div', { class: 'lines' });
      let lastFocused = null;
      d.replies.forEach((line, i) => {
        const ta = h('textarea', { class: 'textarea', rows: '1', id: 'a-reply-' + i, 'aria-label': 'Reply ' + (i + 1), placeholder: i === 0 ? 'e.g. gm {user} ☀️' : 'Another reply', maxlength: '1800' });
        ta.value = line;
        const grow = () => { ta.style.height = 'auto'; ta.style.height = Math.min(200, ta.scrollHeight + 2) + 'px'; };
        ta.addEventListener('input', () => { d.replies[i] = ta.value; grow(); previewIndex = i; drawPreview(); });
        ta.addEventListener('focus', () => { lastFocused = ta; });
        requestAnimationFrame(grow);
        replies.appendChild(h('div', { class: 'line-row' }, h('span', { class: 'line-n', 'aria-hidden': 'true' }, i + 1), ta,
          h('div', { class: 'line-tools' }, h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Remove reply ' + (i + 1), disabled: d.replies.length === 1 && !line, onclick: () => { d.replies.splice(i, 1); if (!d.replies.length) d.replies.push(''); draw(); } }, icon('trash')))));
      });
      const insert = (ph) => {
        const ta = lastFocused && lastFocused.isConnected ? lastFocused : body.querySelector('.line-row textarea');
        if (!ta) return;
        const at = ta.selectionStart || ta.value.length;
        ta.value = ta.value.slice(0, at) + ph + ta.value.slice(ta.selectionEnd || at);
        ta.dispatchEvent(new Event('input')); ta.focus(); ta.selectionStart = ta.selectionEnd = at + ph.length;
      };
      const emojiInput = chipsInput(d.reactions, { id: 'a-reaction', label: 'Add a reaction', placeholder: 'Paste an emoji, then Enter', more: 'Add…', split: true, render: (t) => emojiEl(t), onChange: draw });
      const serverEmoji = h('button', { class: 'btn sm', type: 'button' }, icon('smile'), 'Server emoji');
      serverEmoji.addEventListener('click', async () => {
        const list = await loadEmojis();
        openPicker(serverEmoji, { title: 'Server emoji', placeholder: 'Search the server’s emoji', empty: 'This server has no custom emoji.',
          load: (q) => list.filter((e) => !q || e.name.toLowerCase().includes(q.toLowerCase())).map((e) => ({ id: e.id, label: ':' + e.name + ':', sub: e.animated ? 'animated' : '', lead: h('img', { class: 'emoji-img', src: e.url, alt: '' }), emoji: e })),
          onPick: (it) => { const code = '<' + (it.emoji.animated ? 'a' : '') + ':' + it.emoji.name + ':' + it.emoji.id + '>'; if (!d.reactions.includes(code)) d.reactions.push(code); draw(); } });
      });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('message'), 'Response'),
        h('div', null, h('div', { class: 'label-row' }, h('span', { class: 'label' }, 'Replies'), h('span', { class: 'hint', style: 'margin:0' }, 'One is picked at random. Leave empty to only react.')), replies,
          h('div', { style: 'display:flex;gap:10px;align-items:center;flex-wrap:wrap;margin-top:8px' },
            h('button', { class: 'btn sm', type: 'button', onclick: () => { d.replies.push(''); draw(); const all = body.querySelectorAll('.line-row textarea'); all[all.length - 1].focus(); } }, icon('plus'), 'Add reply'),
            h('div', { class: 'placeholders' }, 'Insert:', [['{user}', 'Mentions the person'], ['{name}', 'Their display name']].map(([ph, tip]) => h('button', { class: 'ph', type: 'button', 'data-tip': tip, onmousedown: (e) => e.preventDefault(), onclick: () => insert(ph) }, ph))))),
        h('div', null, h('span', { class: 'label' }, 'Send it as'), segmented([['reply', 'A reply to the message', 'reply'], ['post', 'A message in the channel', 'message']], d.as_reply ? 'reply' : 'post', 'Send as', (v) => { d.as_reply = v === 'reply'; drawPreview(); })),
        h('div', null, h('label', { class: 'label', for: 'a-reaction' }, 'Reactions'), emojiInput, h('div', { class: 'hint-row' }, h('span', { class: 'hint', style: 'margin:0' }, 'Unicode emoji, or the server’s own.'), serverEmoji))));

      // preview
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('eye'), 'Preview'), preview));

      // timing
      const range = h('input', { class: 'range', type: 'range', min: '1', max: '100', value: d.chance, id: 'a-chance', 'aria-describedby': 'a-chance-words' });
      const words = h('b', { id: 'a-chance-words' }, chanceWords(d.chance));
      range.addEventListener('input', () => { d.chance = +range.value; words.textContent = chanceWords(d.chance); runTest(); });
      const factor = { seconds: 1, minutes: 60, hours: 3600 };
      const cd = h('input', { class: 'input', type: 'number', min: '0', step: '1', id: 'a-cooldown', value: String(d.cooldown_secs / factor[cooldownUnit]), style: 'max-width:110px' });
      const unit = h('select', { class: 'select', 'aria-label': 'Cooldown unit', style: 'max-width:130px' }, ['seconds', 'minutes', 'hours'].map((u) => h('option', { value: u, selected: u === cooldownUnit }, u)));
      const cdHint = h('div', { class: 'hint' });
      const syncCd = () => {
        cooldownUnit = unit.value;
        d.cooldown_secs = Math.max(0, Math.round((parseFloat(cd.value) || 0) * factor[cooldownUnit]));
        const bad = d.cooldown_secs > 604800;
        cd.classList.toggle('invalid', bad);
        cdHint.className = bad ? 'error-text' : 'hint';
        cdHint.textContent = bad ? 'At most 7 days.' : d.cooldown_secs ? 'After it fires in a channel, it stays quiet there for ' + cooldownWords(d.cooldown_secs).replace(' cooldown', '') + '.' : 'Fires on every matching message.';
      };
      cd.addEventListener('input', syncCd); unit.addEventListener('change', syncCd); syncCd();
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('clock'), 'Timing'),
        h('div', null, h('div', { class: 'label-row' }, h('label', { class: 'label', for: 'a-chance' }, 'How often'), words), range,
          h('div', { class: 'range-scale', 'aria-hidden': 'true' }, h('span', null, '1%'), h('span', null, '50%'), h('span', null, 'Every time'))),
        h('div', null, h('label', { class: 'label', for: 'a-cooldown' }, 'Cooldown'), h('div', { style: 'display:flex;gap:8px' }, cd, unit), cdHint)));

      // tester
      const sampleInput = h('input', { class: 'input', id: 'a-sample', value: sample, placeholder: 'Type a message someone might send', autocomplete: 'off' });
      sampleInput.addEventListener('input', () => { sample = sampleInput.value; changed(); });
      const chSel = h('select', { class: 'select', 'aria-label': 'Channel for the test' }, h('option', { value: '' }, 'Any channel'),
        S.channels.filter((c) => c.kind === 'text').map((c) => h('option', { value: c.id, selected: c.id === sampleChannel }, '#' + c.name)));
      chSel.addEventListener('change', () => { sampleChannel = chSel.value; changed(); });
      body.appendChild(h('section', { class: 'form-card tester' }, h('h3', null, icon('flask'), 'Try a message'),
        h('div', { class: 'tester-row' }, sampleInput, chSel), result));

      drawPreview();
      runTest();
      body.scrollTop = scroll;
      if (focusId) { const again = body.querySelector('#' + CSS.escape(focusId)); if (again) again.focus(); }
    }
    draw();
    requestAnimationFrame(() => { const n = body.querySelector('#a-name'); if (n && !existing) n.focus({ preventScroll: true }); });
  }

  // --- house cup ------------------------------------------------------------------------

  // Colour follows the source group, always in this order (validated palette).
  const SOURCE_GROUPS = [
    { id: 'chat', label: 'Chat', icon: '💬', sources: ['chat'] },
    { id: 'voice', label: 'Voice', icon: '🎙️', sources: ['voice'] },
    { id: 'quiz', label: 'Quiz', icon: '🧠', sources: ['quiz'] },
    { id: 'games', label: 'Koto, anagram, cats', icon: '🔤', sources: ['koto', 'anagram', 'cat'] },
    { id: 'arena', label: 'Arena & royale', icon: '⚔️', sources: ['arena', 'royale'] },
    { id: 'snitch', label: 'Snitch', icon: '🪽', sources: ['snitch', 'golden_snitch'] },
    { id: 'weekly', label: 'Weekly posts', icon: '📝', sources: ['weekly'] },
    { id: 'mod', label: 'Mods', icon: '🛡️', sources: ['mod'] },
  ];
  const groupOf = (source) => SOURCE_GROUPS.findIndex((g) => g.sources.includes(source));
  const PERIODS = [['today', 'Today'], ['week', 'This week'], ['month', 'This month'], ['last_month', 'Last month'], ['all', 'All time']];
  const cup = { period: 'month', data: null, timer: null, table: false, tab: 0, updated: 0, loading: false, error: null };

  function ordinal(n) { return n + (n % 10 === 1 && n % 100 !== 11 ? 'st' : n % 10 === 2 && n % 100 !== 12 ? 'nd' : n % 10 === 3 && n % 100 !== 13 ? 'rd' : 'th'); }
  function fmtPoints(n) { return numberFmt.format(n); }

  function renderHouses(page) {
    document.title = 'House Cup · Loduchand';
    const live = h('span', { class: 'live-pill', 'aria-live': 'polite' });
    page.appendChild(pageHead('House Cup', 'The house points race as it happens. Updates every 20 seconds while this tab is open.', null, h('span', { class: 'feature-icon', 'aria-hidden': 'true' }, '🏆')));
    page.appendChild(cupTabs('standings'));
    page.appendChild(h('div', { class: 'toolbar cup-toolbar' },
      segmented(PERIODS, cup.period, 'Period', (v) => { cup.period = v; cup.data = null; drawBody(); refresh(); }), h('span', { class: 'grow' }), live));
    const bodyEl = h('div', { class: 'cup-body' });
    page.appendChild(bodyEl);

    const setLive = () => {
      clear(live);
      if (cup.error) { live.className = 'live-pill err'; append(live, [icon('alert'), 'Can’t update: ' + cup.error]); return; }
      if (document.hidden) { live.className = 'live-pill paused'; append(live, [icon('pause'), 'Paused while hidden']); return; }
      live.className = 'live-pill';
      append(live, [h('span', { class: 'live-dot', 'aria-hidden': 'true' }), 'Live', cup.updated ? h('span', { class: 'live-time' }, ' · updated ' + fmtTime.format(new Date(cup.updated)) + ' IST') : null]);
    };
    const refresh = async () => {
      if (!page.isConnected) { clearInterval(cup.timer); return; }
      // A hidden tab skips the timed updates, but still loads what it has never shown.
      if (cup.loading || (document.hidden && cup.data)) { setLive(); return; }
      cup.loading = true;
      const period = cup.period;
      try {
        const data = await api('GET', '/houses?period=' + period);
        if (period !== cup.period || !page.isConnected) return;
        const before = cup.data;
        cup.data = data; cup.updated = Date.now(); cup.error = null;
        drawBody(before);
      } catch (e) { cup.error = e.message; }
      finally { cup.loading = false; setLive(); }
    };
    const drawBody = (before) => {
      clear(bodyEl);
      const data = cup.data;
      if (!data) { bodyEl.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading the standings…')); return; }
      bodyEl.appendChild(cupCards(data, before));
      bodyEl.appendChild(cupShare(data));
      bodyEl.appendChild(h('div', { class: 'two-col cup-cols' }, cupSources(data), cupToday(data)));
      bodyEl.appendChild(cupScorers(data));
      bodyEl.appendChild(cupFeed(data));
    };
    drawBody();
    clearInterval(cup.timer);
    cup.timer = setInterval(refresh, 20000);
    setLive();
    refresh();
    renderHouses.onVisible = () => { if (page.isConnected && !document.hidden) refresh(); else if (page.isConnected) setLive(); };
  }
  document.addEventListener('visibilitychange', () => { if (renderHouses.onVisible) renderHouses.onVisible(); });

  function houseStyle(hs) { return '--house:' + (hs.colour || 'var(--accent)') + ';--house-2:' + (hs.secondary || 'var(--accent)'); }

  function cupCards(data, before) {
    const sorted = data.houses.slice().sort((a, b) => b.total - a.total);
    const second = sorted[1] ? sorted[1].total : 0;
    return h('div', { class: 'cup-cards' }, data.houses.map((hs) => {
      const prev = before && before.houses.find((x) => x.key === hs.key);
      const bumped = prev && prev.total !== hs.total;
      const leader = hs.rank === 1;
      const gap = leader ? (hs.total - second > 0 ? 'Leading by ' + fmtPoints(hs.total - second) : 'Level at the top') : fmtPoints(hs.gap) + ' behind the leader';
      return h('article', { class: 'cup-card' + (leader ? ' leader' : ''), style: houseStyle(hs), 'aria-label': hs.name + ', ' + ordinal(hs.rank) + ', ' + hs.total + ' points' },
        h('div', { class: 'cup-top' }, h('span', { class: 'crest', 'aria-hidden': 'true' }, hs.crest),
          h('div', { class: 'grow' }, h('h3', null, hs.name), h('span', { class: 'cup-members' }, numberFmt.format(hs.members) + ' members')),
          h('span', { class: 'rank' + (leader ? ' first' : '') }, leader ? [icon('trophy'), '1st'] : ordinal(hs.rank))),
        h('div', { class: 'cup-total' + (bumped ? ' bump' : '') }, fmtPoints(hs.total), h('small', null, ' points')),
        h('div', { class: 'cup-gap' }, gap),
        h('div', { class: 'cup-captain' }, hs.captain ? [avatar(hs.captain.avatar, hs.captain.name || '?', 'xs'), h('span', null, hs.captain.name || 'Former member'), h('small', null, 'Captain')] : h('small', null, 'No captain named')));
    }));
  }

  function cupShare(data) {
    const positive = data.houses.map((hs) => Math.max(0, hs.total));
    const sum = positive.reduce((a, b) => a + b, 0);
    const bar = h('div', { class: 'share-bar', role: 'img', 'aria-label': 'Share of points: ' + data.houses.map((hs, i) => hs.name + ' ' + (sum ? Math.round((positive[i] / sum) * 100) : 0) + '%').join(', ') });
    const labels = h('div', { class: 'share-labels' });
    data.houses.forEach((hs, i) => {
      const pct = sum ? (positive[i] / sum) * 100 : 25;
      bar.appendChild(h('span', { style: houseStyle(hs) + ';flex-grow:' + (sum ? pct : 1) }));
      labels.appendChild(h('span', { class: 'share-label', style: houseStyle(hs) }, h('i', { 'aria-hidden': 'true' }), hs.name, h('b', null, sum ? Math.round(pct) + '%' : '—')));
    });
    return h('div', { class: 'card share' }, h('div', { class: 'share-head' }, h('h2', null, 'Share of the cup'), h('span', { class: 'sub' }, fmtPoints(sum) + ' points so far ' + PERIODS.find((p) => p[0] === cup.period)[1].toLowerCase())), bar, labels);
  }

  let tip = null;
  function showTip(e, lines) {
    if (!tip) { tip = h('div', { class: 'viz-tip', role: 'tooltip' }); document.body.appendChild(tip); }
    clear(tip);
    lines.forEach((l, i) => tip.appendChild(i === 0 ? h('b', null, l) : h('div', null, l)));
    tip.hidden = false;
    const x = Math.min(window.innerWidth - tip.offsetWidth - 8, e.clientX + 14);
    const y = e.clientY - tip.offsetHeight - 12 < 8 ? e.clientY + 16 : e.clientY - tip.offsetHeight - 12;
    tip.style.left = x + 'px'; tip.style.top = y + 'px';
  }
  function hideTip() { if (tip) tip.hidden = true; }

  function groupTotals(list) {
    const totals = SOURCE_GROUPS.map(() => ({ points: 0, parts: [] }));
    (list || []).forEach((s) => { const g = groupOf(s.source); if (g >= 0) { totals[g].points += s.points; totals[g].parts.push(s); } });
    return totals;
  }
  function sourceName(key) { const s = (cup.data && cup.data.sources || []).find((x) => x.key === key); return s ? s.icon + ' ' + s.label : key; }

  function cupSources(data) {
    const rows = data.houses.map((hs) => ({ hs, groups: groupTotals(hs.by_source) }));
    const max = Math.max(1, ...rows.map((r) => r.groups.reduce((a, g) => a + Math.max(0, g.points), 0)));
    const present = SOURCE_GROUPS.map((g, i) => rows.some((r) => r.groups[i].points > 0));
    const legend = h('div', { class: 'legend' }, SOURCE_GROUPS.map((g, i) => present[i] ? h('span', { class: 'legend-item' }, h('i', { style: 'background:var(--s' + (i + 1) + ')' }), g.label) : null));
    const toggle = h('button', { class: 'btn sm ghost', type: 'button', 'aria-pressed': cup.table ? 'true' : 'false', onclick: () => { cup.table = !cup.table; const fresh = cupSources(data); el.replaceWith(fresh); } }, icon(cup.table ? 'chart' : 'table'), cup.table ? 'Chart' : 'Table');
    let content;
    if (cup.table) {
      content = h('div', { class: 'table-wrap' }, h('table', { class: 'mini-table' },
        h('thead', null, h('tr', null, h('th', null, 'Source'), data.houses.map((hs) => h('th', { class: 'num' }, hs.crest + ' ' + hs.name)))),
        h('tbody', null, SOURCE_GROUPS.map((g, i) => present[i] ? h('tr', null, h('td', null, g.icon + ' ' + g.label), rows.map((r) => h('td', { class: 'num' }, r.groups[i].points ? fmtPoints(r.groups[i].points) : '—'))) : null),
          h('tr', { class: 'total' }, h('td', null, 'Total'), data.houses.map((hs) => h('td', { class: 'num' }, fmtPoints(hs.total)))))));
    } else {
      content = h('div', { class: 'stack-chart' }, legend, rows.map(({ hs, groups }) => {
        const net = groups.reduce((a, g) => a + Math.max(0, g.points), 0);
        const track = h('div', { class: 'stack-track', style: 'width:' + Math.max(net ? 2 : 0, (net / max) * 100) + '%' });
        groups.forEach((g, i) => {
          if (g.points <= 0) return;
          const seg = h('span', { class: 'stack-seg', style: 'flex-grow:' + g.points + ';background:var(--s' + (i + 1) + ')', tabindex: '0', 'aria-label': hs.name + ', ' + SOURCE_GROUPS[i].label + ': ' + g.points + ' points' });
          const lines = () => [hs.name + ' · ' + SOURCE_GROUPS[i].label, fmtPoints(g.points) + ' points (' + Math.round((g.points / Math.max(1, net)) * 100) + '%)'].concat(g.parts.length > 1 ? g.parts.map((p) => sourceName(p.source) + ': ' + fmtPoints(p.points)) : []);
          seg.addEventListener('mousemove', (e) => showTip(e, lines()));
          seg.addEventListener('mouseleave', hideTip);
          seg.addEventListener('focus', () => { const r = seg.getBoundingClientRect(); showTip({ clientX: r.left + r.width / 2, clientY: r.top }, lines()); });
          seg.addEventListener('blur', hideTip);
          track.appendChild(seg);
        });
        return h('div', { class: 'stack-row' }, h('span', { class: 'stack-label' }, h('span', { 'aria-hidden': 'true' }, hs.crest), hs.name),
          h('div', { class: 'stack-bar' }, track, h('span', { class: 'stack-value' }, fmtPoints(hs.total))));
      }));
    }
    const el = card('cup-sources', 'Where the points came from', PERIODS.find((p) => p[0] === cup.period)[1], h('div', { class: 'card-body pad-sm' }, content), { actions: toggle });
    return el;
  }

  function cupToday(data) {
    const rows = data.houses.map((hs) => groupTotals(hs.today_by_source));
    const present = SOURCE_GROUPS.map((g, i) => rows.some((r) => r[i].points !== 0));
    const max = Math.max(1, ...rows.flatMap((r) => r.map((g) => g.points)));
    const totals = data.houses.map((hs) => (hs.today_by_source || []).reduce((a, s) => a + s.points, 0));
    const body = present.some(Boolean)
      ? h('div', { class: 'table-wrap' }, h('table', { class: 'mini-table heat' },
        h('thead', null, h('tr', null, h('th', null, h('span', { class: 'sr' }, 'Source')), data.houses.map((hs) => h('th', { class: 'num', title: hs.name, style: houseStyle(hs) }, h('span', { class: 'th-crest' }, hs.crest), h('span', { class: 'sr' }, hs.name))))),
        h('tbody', null, SOURCE_GROUPS.map((g, i) => present[i] ? h('tr', null, h('td', null, h('span', { 'aria-hidden': 'true' }, g.icon + ' '), g.label),
          rows.map((r) => { const v = r[i].points; return h('td', { class: 'num', style: '--heat:' + Math.max(0, Math.round((v / max) * 100)) + '%' }, v ? fmtPoints(v) : h('span', { class: 'muted' }, '·')); })) : null),
          h('tr', { class: 'total' }, h('td', null, 'Today'), totals.map((t) => h('td', { class: 'num' }, fmtPoints(t)))))))
      : h('div', { class: 'empty' }, icon('clock'), h('p', null, 'No points yet today (India time).'));
    return card('cup-today', 'Today so far', 'By source, India time', body);
  }

  function cupScorers(data) {
    const narrow = window.innerWidth < 900;
    const tabs = segmented(data.houses.map((hs, i) => [String(i), hs.crest + ' ' + hs.name]), String(cup.tab), 'House', (v) => { cup.tab = +v; cols.querySelectorAll('.scorer-col').forEach((c, i) => c.classList.toggle('shown', i === cup.tab)); });
    const cols = h('div', { class: 'scorers' }, data.houses.map((hs, i) => h('section', { class: 'scorer-col' + (i === cup.tab ? ' shown' : ''), style: houseStyle(hs), 'aria-label': hs.name + ' top scorers' },
      h('h3', null, h('span', { 'aria-hidden': 'true' }, hs.crest), hs.name),
      hs.top.length ? h('ol', null, hs.top.map((t, n) => {
        const g = groupOf(t.main_source);
        return h('li', null, h('span', { class: 'pos' }, n + 1), avatar(t.avatar, t.name || '?', 'xs'),
          h('span', { class: 'who' }, t.name || h('i', null, 'Former member')),
          h('span', { class: 'src', title: 'Mostly ' + sourceName(t.main_source), 'aria-label': 'mostly ' + sourceName(t.main_source) }, g >= 0 ? SOURCE_GROUPS[g].icon : '•'),
          h('b', null, fmtPoints(t.points)));
      })) : h('p', { class: 'empty-small' }, 'Nobody has scored yet.'))));
    return card('cup-scorers', 'Top scorers', 'Muggles are left out', [narrow ? h('div', { class: 'scorer-tabs' }, tabs) : null, cols]);
  }

  function cupFeed(data) {
    const list = h('ul', { class: 'feed' });
    if (!data.feed.length) list.appendChild(h('li', { class: 'empty' }, h('p', null, 'No points have been awarded yet.')));
    data.feed.forEach((row) => {
      const hs = data.houses.find((x) => x.key === row.house) || { name: row.house, crest: '', colour: null };
      const g = groupOf(row.source);
      const who = row.member ? (row.member.name || 'Former member') : hs.name + ' (house award)';
      list.appendChild(h('li', { class: 'feed-row' },
        h('span', { class: 'feed-icon', style: g >= 0 ? '--src:var(--s' + (g + 1) + ')' : '', 'aria-hidden': 'true' }, row.source_icon || '•'),
        h('div', { class: 'feed-main' },
          h('div', null, row.member ? avatar(row.member.avatar, who, 'xs') : null, h('b', null, who), h('span', { class: 'house-chip', style: houseStyle(hs) }, hs.crest + ' ' + hs.name), h('span', { class: 'feed-src' }, row.source_label)),
          row.reason ? h('small', null, row.reason) : null),
        h('div', { class: 'feed-side' }, h('b', { class: 'pts' + (row.points < 0 ? ' neg' : '') }, (row.points > 0 ? '+' : '') + row.points), h('small', { title: fmtFull.format(new Date(row.ts * 1000)) + ' IST' }, ago(row.ts)))));
    });
    return card('cup-feed', 'Latest points', 'The last 50 across all houses', list);
  }

  // --- bot behaviour ----------------------------------------------------------------------

  const TONE_PRESETS = [
    ['Friendly Hinglish banter', 'Tone: friendly Hinglish banter. Talk like a friend in the group chat, mixing Hindi and English the way members do ("haan bhai", "scene kya hai"). Light teasing is fine; keep it warm, never mean.'],
    ['Calm and helpful', 'Tone: calm and helpful. Be patient and clear, explain step by step when someone asks, and keep a steady, kind voice even when a chat gets heated.'],
    ['Short replies', 'Keep replies short: one to three sentences unless someone asks for detail. No long lists or essays in casual chat.'],
    ['No roasting', "Don't roast, insult or make fun of members, even if they ask for it or others are doing it. Joke with people, not about them."],
  ];
  const CHANCE_STOPS = [0, 1, 2, 3, 5, 8, 10, 15, 20, 30, 50, 75, 100];
  function chattinessWords(pct) {
    if (pct <= 0) return ['Only when talked to', 'It answers mentions and replies, and never joins in on its own.'];
    const oneIn = Math.round(100 / pct);
    const tail = pct >= 100 ? 'It considers answering every message it reads.' : 'About 1 in ' + oneIn + ' messages that weren’t meant for it get a reply.';
    if (pct <= 2) return ['Rarely joins in', tail];
    if (pct <= 5) return ['Joins in now and then', tail];
    if (pct <= 15) return ['Chatty', tail];
    if (pct <= 40) return ['Very chatty', tail];
    return ['Talks over everyone', tail];
  }

  const agentState = { base: null, draft: null, notes: {}, loading: false, error: null, tab: 'system_prompt', undo: { system_prompt: [], core: [] } };
  const AGENT_TEXT = ['name', 'description', 'system_prompt', 'core', 'model'];

  function agentDirty() {
    const a = agentState;
    if (!a.base || !a.draft) return [];
    return ['name', 'description', 'system_prompt', 'core', 'model', 'thinking_depth', 'silent_read_initiative_chance', 'max_tokens'].filter((f) => JSON.stringify(a.base[f]) !== JSON.stringify(a.draft[f]));
  }

  async function renderAgent(page) {
    document.title = 'Bot behaviour · Loduchand';
    page.appendChild(pageHead('Bot behaviour', 'How Loduchand talks when people chat with it: its instructions, notes, how often it joins in, and its model.',
      h('button', { class: 'btn', type: 'button', onclick: restartFlow }, icon('restart'), 'Restart bot'), h('span', { class: 'feature-icon', 'aria-hidden': 'true' }, '🤖')));
    page.appendChild(h('div', { class: 'banner inline info' }, icon('restart'),
      h('p', null, h('b', null, 'Changes apply after a restart. '), h('span', null, 'Saving stores them straight away, but the chat side of the bot reads these settings when it starts. Quiz drafting and the weekly scan pick up a new model at once.'))));
    const holder = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading the bot’s settings…'));
    page.appendChild(holder);
    S.guard = () => agentDirty().length;
    S.discard = () => { agentState.base = null; agentState.draft = null; };
    if (!agentState.base || agentDirty().length === 0) {
      try {
        const got = await api('GET', '/agent');
        agentState.base = got.settings; agentState.notes = got.notes || {};
        agentState.draft = JSON.parse(JSON.stringify(got.settings));
        agentState.undo = { system_prompt: [], core: [] };
      } catch (e) {
        clear(holder).appendChild(h('div', { class: 'card empty' }, icon('alert'), h('h3', null, 'Can’t load the bot’s settings'), h('p', null, e.message)));
        return;
      }
    }
    if (!page.isConnected) return;
    clear(holder);
    drawAgent(holder);
  }

  function drawAgent(holder) {
    const a = agentState, d = a.draft;
    clear(holder);
    const update = () => updateAgentBar();

    // identity
    const name = h('input', { class: 'input', id: 'g-name', value: d.name, maxlength: '64' });
    const desc = h('input', { class: 'input', id: 'g-desc', value: d.description, maxlength: '300', placeholder: 'e.g. the resident bot of the MLCI server' });
    const reads = h('p', { class: 'reads' });
    const drawReads = () => { clear(reads); append(reads, ['The bot reads: ', h('q', null, 'You are ' + (d.name || '…') + ', ' + (d.description || 'a Digital Steward') + '.')]); };
    name.addEventListener('input', () => { d.name = name.value; drawReads(); update(); });
    desc.addEventListener('input', () => { d.description = desc.value; drawReads(); update(); });
    drawReads();
    holder.appendChild(card('agent-identity', 'Identity', 'Who the bot says it is', h('div', { class: 'card-body pad' },
      h('div', { class: 'form-grid' }, h('div', null, h('label', { class: 'label', for: 'g-name' }, 'Name'), name), h('div', null, h('label', { class: 'label', for: 'g-desc' }, 'Description'), desc)), reads)));

    // personality
    const editorWrap = h('div', { class: 'prompt-wrap' });
    const drawEditor = () => {
      clear(editorWrap);
      const field = a.tab;
      const ta = h('textarea', { class: 'textarea prompt-editor', id: 'g-text', spellcheck: 'true', 'aria-label': field === 'core' ? 'Core notes' : 'System prompt', 'aria-describedby': 'g-text-help' });
      ta.value = d[field];
      const count = h('span', { class: 'text-count' });
      const undoBtn = h('button', { class: 'btn sm ghost', type: 'button', hidden: !a.undo[field].length, onclick: () => {
        const prev = a.undo[field].pop();
        if (prev === undefined) return;
        d[field] = prev; drawEditor(); update(); toast('Undid the last tone insert', 'info');
      } }, icon('reset'), 'Undo insert');
      const drawCount = () => {
        const text = ta.value;
        const words = (text.match(/\S+/g) || []).length;
        count.textContent = numberFmt.format(text.length) + ' characters · ' + numberFmt.format(words) + ' words' + (text !== a.base[field] ? ' · edited' : '');
        count.classList.toggle('edited', text !== a.base[field]);
      };
      ta.addEventListener('input', () => { d[field] = ta.value; drawCount(); update(); });
      drawCount();
      const presets = h('div', { class: 'presets' }, h('span', { class: 'hint', style: 'margin:0' }, 'Add a tone:'),
        TONE_PRESETS.map(([label, text]) => h('button', { class: 'ph preset', type: 'button', 'data-tip': text, onmousedown: (e) => e.preventDefault(), onclick: () => {
          a.undo[field].push(d[field]);
          const at = document.activeElement === ta ? ta.selectionEnd : ta.value.length;
          const beforeText = ta.value.slice(0, at);
          const pad = (beforeText && !beforeText.endsWith('\n\n') ? (beforeText.endsWith('\n') ? '\n' : '\n\n') : '');
          const insert = pad + text + (ta.value.slice(at).startsWith('\n') || at === ta.value.length ? '' : '\n\n');
          ta.focus();
          ta.setSelectionRange(at, at);
          const done = document.execCommand && document.execCommand('insertText', false, insert);
          if (!done) { ta.value = ta.value.slice(0, at) + insert + ta.value.slice(at); }
          d[field] = ta.value;
          ta.setSelectionRange(at + pad.length, at + insert.length);
          drawCount(); update();
          undoBtn.hidden = false;
          toast('Added “' + label + '”. Edit it, or undo.', 'info');
        } }, icon('plus'), label)));
      append(editorWrap, [
        h('p', { class: 'hint', id: 'g-text-help', style: 'margin:0 0 10px' }, field === 'core'
          ? 'The bot’s own notes about people, running jokes and lessons. It rewrites these itself too: if it does while you edit, you’ll be asked to reload before saving.'
          : 'The main instructions: who the bot is, how it should talk, and what to avoid.'),
        presets, ta, h('div', { class: 'editor-foot' }, count, h('span', { class: 'grow' }), undoBtn),
      ]);
    };
    const tabs = segmented([['system_prompt', 'System prompt', 'message'], ['core', 'Core notes', 'edit']], a.tab, 'Which text', (v) => { a.tab = v; drawEditor(); });
    drawEditor();
    holder.appendChild(card('agent-tone', 'Personality & tone', 'Tone presets add a paragraph you can edit; nothing is replaced', h('div', { class: 'card-body pad' }, h('div', { style: 'margin-bottom:12px' }, tabs), editorWrap)));

    // chattiness
    const pct = Math.round(d.silent_read_initiative_chance * 1000) / 10;
    const nearest = CHANCE_STOPS.reduce((best, s, i) => (Math.abs(s - pct) < Math.abs(CHANCE_STOPS[best] - pct) ? i : best), 0);
    const range = h('input', { class: 'range', type: 'range', min: '0', max: String(CHANCE_STOPS.length - 1), step: '1', value: nearest, id: 'g-chance', 'aria-describedby': 'g-chance-words' });
    const exact = h('input', { class: 'input', type: 'number', min: '0', max: '100', step: '0.5', value: String(pct), 'aria-label': 'Exact percent' });
    const title = h('b', { class: 'chatty-title' });
    const sub = h('span', { class: 'hint', style: 'margin:0', id: 'g-chance-words' });
    const drawWords = () => { const p = Math.round(d.silent_read_initiative_chance * 1000) / 10; const [t, s2] = chattinessWords(p); title.textContent = t; sub.textContent = s2; range.setAttribute('aria-valuetext', p + '%, ' + t); };
    range.addEventListener('input', () => { const v = CHANCE_STOPS[+range.value]; d.silent_read_initiative_chance = v / 100; exact.value = String(v); drawWords(); update(); });
    exact.addEventListener('input', () => { const v = Math.max(0, Math.min(100, parseFloat(exact.value) || 0)); d.silent_read_initiative_chance = Math.round(v * 10) / 1000; const i = CHANCE_STOPS.reduce((b, s, j) => (Math.abs(s - v) < Math.abs(CHANCE_STOPS[b] - v) ? j : b), 0); range.value = i; drawWords(); update(); });
    drawWords();
    holder.appendChild(card('agent-chatty', 'Chattiness', 'Joining conversations it wasn’t asked into', h('div', { class: 'card-body pad chatty' },
      h('div', { class: 'label-row' }, title, h('div', { class: 'input-group', style: 'width:130px' }, exact, h('span', { class: 'addon' }, '%'))), sub,
      range, h('div', { class: 'range-scale', 'aria-hidden': 'true' }, h('span', null, 'Never'), h('span', null, '5%'), h('span', null, '20%'), h('span', null, 'Always')))));

    // model & limits
    const model = h('input', { class: 'input mono', id: 'g-model', value: d.model, spellcheck: 'false', autocomplete: 'off' });
    model.addEventListener('input', () => { d.model = model.value.trim(); update(); });
    const tokens = h('input', { class: 'input', type: 'number', id: 'g-tokens', min: '1', max: '1000000', value: d.max_tokens == null ? '' : String(d.max_tokens), placeholder: 'Provider default' });
    tokens.addEventListener('input', () => { const v = tokens.value.trim(); d.max_tokens = v === '' ? null : Math.round(+v); update(); });
    const depth = h('input', { class: 'input', type: 'number', id: 'g-depth', min: '1', max: '64', value: String(d.thinking_depth) });
    depth.addEventListener('input', () => { d.thinking_depth = Math.round(+depth.value || 0); update(); });
    holder.appendChild(card('agent-model', 'Model & limits', null, h('div', { class: 'card-body pad' },
      h('div', { class: 'form-grid' },
        h('div', { class: 'full' }, h('label', { class: 'label', for: 'g-model' }, 'Model'), model, h('div', { class: 'hint' }, 'The name your AI provider uses, like provider/model-name. A name it doesn’t know stops chat replies, so copy it exactly.')),
        h('div', null, h('label', { class: 'label', for: 'g-tokens' }, 'Max reply length'), h('div', { class: 'input-group' }, tokens, h('span', { class: 'addon' }, 'tokens')), h('div', { class: 'hint' }, 'Empty uses the provider’s default. About 750 words per 1,000 tokens.')),
        h('div', null, h('label', { class: 'label', for: 'g-depth' }, 'Thinking depth'), h('div', { class: 'input-group' }, depth, h('span', { class: 'addon' }, 'steps')), h('div', { class: 'hint' }, 'How many tool steps one reply may take, 1 to 64. More can help hard questions and costs more.'))))));
    updateAgentBar();
  }

  let agentBar = null;
  function agentProblem() {
    const d = agentState.draft;
    if (!d.name.trim()) return 'The name can’t be empty.';
    if (!d.core.trim()) return 'The core notes can’t be empty.';
    if (!d.model || /\s/.test(d.model)) return 'The model is a name without spaces.';
    if (!(d.thinking_depth >= 1 && d.thinking_depth <= 64)) return 'Thinking depth is 1 to 64.';
    if (d.max_tokens !== null && !(d.max_tokens >= 1 && d.max_tokens <= 1000000)) return 'Max reply length is empty or 1 to 1,000,000.';
    return null;
  }
  function updateAgentBar() {
    const changed = agentDirty();
    if (!changed.length || route().name !== 'agent') { if (agentBar) { agentBar.remove(); agentBar = null; } return; }
    if (!agentBar) { agentBar = h('div', { class: 'savebar', role: 'region', 'aria-label': 'Unsaved changes' }); document.body.appendChild(agentBar); }
    clear(agentBar);
    const problem = agentProblem();
    append(agentBar, [
      h('p', null, h('span', { class: 'dot' }), plural(changed.length, 'unsaved change')),
      problem ? h('span', { class: 'error-text', style: 'margin:0 8px 0 0' }, icon('alert'), problem) : null,
      h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { agentState.draft = JSON.parse(JSON.stringify(agentState.base)); agentState.undo = { system_prompt: [], core: [] }; rerender(); } }, 'Discard'),
      h('button', { class: 'btn sm primary', type: 'button', disabled: !!problem, onclick: reviewAgent }, 'Review & save'),
    ]);
  }

  const AGENT_LABELS = { name: 'Name', description: 'Description', system_prompt: 'System prompt', core: 'Core notes', model: 'Model', thinking_depth: 'Thinking depth', silent_read_initiative_chance: 'Chiming in', max_tokens: 'Max reply length' };

  /** Line diff: [{type:'same'|'add'|'del', text}], with runs of unchanged lines folded. */
  function lineDiff(a, b) {
    const x = a.split('\n'), y = b.split('\n');
    if (x.length * y.length > 4000000) return [{ type: 'del', text: a }, { type: 'add', text: b }];
    const n = x.length, m = y.length;
    const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) lcs[i][j] = x[i] === y[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    const out = [];
    let i = 0, j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && x[i] === y[j]) { out.push({ type: 'same', text: x[i] }); i++; j++; }
      else if (j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j])) { out.push({ type: 'add', text: y[j] }); j++; }
      else { out.push({ type: 'del', text: x[i] }); i++; }
    }
    return out;
  }

  function diffView(a, b) {
    const rows = lineDiff(a || '', b || '');
    const el = h('div', { class: 'diff', role: 'table', 'aria-label': 'Changes' });
    const near = rows.map((r, i) => r.type !== 'same' || rows.slice(Math.max(0, i - 2), i + 3).some((x) => x.type !== 'same'));
    let skipped = 0;
    rows.forEach((r, i) => {
      if (!near[i]) { skipped++; return; }
      if (skipped) { el.appendChild(h('div', { class: 'diff-skip' }, '⋯ ' + plural(skipped, 'unchanged line'))); skipped = 0; }
      el.appendChild(h('div', { class: 'diff-line ' + r.type }, h('span', { class: 'diff-mark', 'aria-label': r.type === 'add' ? 'added' : r.type === 'del' ? 'removed' : '' }, r.type === 'add' ? '+' : r.type === 'del' ? '−' : ' '), h('span', null, r.text || ' ')));
    });
    if (skipped) el.appendChild(h('div', { class: 'diff-skip' }, '⋯ ' + plural(skipped, 'unchanged line')));
    return el;
  }

  async function reviewAgent() {
    const a = agentState, changed = agentDirty();
    const show = (f, v) => (f === 'silent_read_initiative_chance' ? Math.round(v * 1000) / 10 + '%' : f === 'max_tokens' ? (v == null ? 'provider default' : numberFmt.format(v) + ' tokens') : String(v));
    const body = h('div', { class: 'review' }, changed.map((f) => h('div', { class: 'review-item' },
      h('h4', null, AGENT_LABELS[f]),
      f === 'system_prompt' || f === 'core' || f === 'description'
        ? diffView(a.base[f], a.draft[f])
        : h('div', { class: 'change-diff' }, h('span', { class: 'val old' }, show(f, a.base[f])), h('span', { class: 'arrow' }, '→'), h('span', { class: 'val' }, show(f, a.draft[f]))))),
      h('p', { class: 'hint' }, 'Saved now; the chat side uses it after the bot restarts.'));
    const ok = await confirmDialog({ title: 'Save ' + plural(changed.length, 'change') + ' to the bot?', body, confirm: 'Save changes', wide: true });
    if (!ok) return;
    const patch = {};
    changed.forEach((f) => { patch[f] = a.draft[f]; });
    patch.base = {};
    ['core', 'system_prompt', 'description', 'name'].forEach((f) => { if (f in patch) patch.base[f] = a.base[f]; });
    try {
      const res = await api('PUT', '/agent', patch);
      a.base = res.settings;
      a.draft = JSON.parse(JSON.stringify(res.settings));
      a.undo = { system_prompt: [], core: [] };
      if (res.restart_needed) { S.restartPending = true; renderBanner(); }
      toast('Saved. Restart the bot to use it in chat.');
      refreshAudit();
      rerender();
    } catch (e) {
      if (e.status === 409) {
        const reload = await confirmDialog({ title: 'The bot changed this meanwhile', icon: 'alert', body: e.message + ' Your edits stay on this page until you reload.', confirm: 'Reload latest', cancel: 'Keep editing' });
        if (reload) { a.base = null; a.draft = null; rerender(); }
      } else toast(e.message, 'error');
    }
  }

  // --- scorers today ------------------------------------------------------------------------

  const scorersState = { day: 'today', house: 'all', q: '', data: null, timer: null, updated: 0, loading: false, error: null };

  function cupTabs(active) {
    return h('nav', { class: 'page-tabs', 'aria-label': 'House Cup views' },
      h('a', { href: '#/houses', 'aria-current': active === 'standings' ? 'page' : null }, icon('trophy'), 'Standings'),
      h('a', { href: '#/houses/scorers', 'aria-current': active === 'scorers' ? 'page' : null }, icon('zap'), 'Scorers today'));
  }

  /** One activity chip: progress towards the chat/voice point, a capped source, or an extra. */
  function actChip(c) {
    const pts = c.points || 0;
    let cls = 'act', text, pct = 0, title;
    if (c.kind === 'progress') {
      pct = c.target ? Math.min(1, c.count / c.target) : 0;
      if (c.reached) { cls += ' reached'; text = [icon('check'), (c.key === 'chat' ? 'chat point' : 'voice point')]; }
      else text = c.count + '/' + c.target + ' ' + c.unit;
      if (!c.count && !pts) cls += ' zero';
      title = c.label + ': ' + c.count + ' of ' + c.target + ' ' + (c.unit === 'msgs' ? 'messages' : 'minutes') + (c.reached ? ', point earned' : '');
    } else if (c.kind === 'capped') {
      pct = c.cap ? Math.min(1, pts / c.cap) : 0;
      if (c.reached) { cls += ' reached'; text = [icon('check'), 'max ' + c.cap]; }
      else text = pts + (c.cap ? '/' + c.cap : '');
      if (!pts) cls += ' zero';
      title = c.label + ': ' + pts + (c.cap ? ' of ' + c.cap + ' today' : ' today') + (c.reached ? ', daily limit reached' : '');
    } else if (c.kind === 'weekly') {
      cls += ' extra';
      text = (pts ? '+' + pts + ' · ' : '') + c.week + ' this week';
      title = 'Weekly posts: ' + pts + ' today, ' + c.week + ' this week (limit ' + c.cap_per_channel + ' per channel per week)';
    } else {
      cls += ' extra';
      text = (pts > 0 ? '+' : '') + pts;
      title = c.label + ': ' + pts + ' today (no daily limit)';
    }
    const el = h('span', { class: cls, 'data-tip': title, 'aria-label': title, tabindex: '0' },
      h('span', { class: 'act-icon', 'aria-hidden': 'true' }, c.icon), h('span', { class: 'act-text' }, text));
    if (c.kind === 'progress' || c.kind === 'capped') el.appendChild(h('span', { class: 'act-bar', 'aria-hidden': 'true' }, h('i', { style: 'width:' + Math.round(pct * 100) + '%' })));
    return el;
  }

  function scorersLegend() {
    return h('details', { class: 'act-legend-wrap', open: window.innerWidth > 640 }, h('summary', null, 'What the chips mean'), h('div', { class: 'act-legend' },
      h('span', null, h('span', { class: 'act' }, h('span', { class: 'act-icon' }, '🧠'), h('span', { class: 'act-text' }, '4/6'), h('span', { class: 'act-bar' }, h('i', { style: 'width:66%' }))), ' points today of the daily limit'),
      h('span', null, h('span', { class: 'act reached' }, h('span', { class: 'act-icon' }, '🧠'), h('span', { class: 'act-text' }, icon('check'), 'max 6')), ' limit reached'),
      h('span', null, h('span', { class: 'act' }, h('span', { class: 'act-icon' }, '💬'), h('span', { class: 'act-text' }, '14/20 msgs'), h('span', { class: 'act-bar' }, h('i', { style: 'width:70%' }))), ' on the way to the chat or voice point'),
      h('span', null, h('span', { class: 'act zero' }, h('span', { class: 'act-icon' }, '⚔️'), h('span', { class: 'act-text' }, '0/3')), ' nothing yet')));
  }

  function renderScorers(page) {
    document.title = 'Scorers today · Loduchand';
    const st = scorersState;
    page.appendChild(pageHead('House Cup', 'Who has scored today, and who has hit the daily limit for each game. Updates every 20 seconds.', null, h('span', { class: 'feature-icon', 'aria-hidden': 'true' }, '🏆')));
    page.appendChild(cupTabs('scorers'));
    const live = h('span', { class: 'live-pill', 'aria-live': 'polite' });
    const search = h('input', { type: 'search', placeholder: 'Find a member', 'aria-label': 'Find a member', value: st.q });
    const houseOpts = [['all', 'All houses']].concat(((cup.data && cup.data.houses) || [
      { key: 'gryffindor', crest: '🦁', name: 'Gryffindor' }, { key: 'slytherin', crest: '🐍', name: 'Slytherin' },
      { key: 'ravenclaw', crest: '🦅', name: 'Ravenclaw' }, { key: 'hufflepuff', crest: '🦡', name: 'Hufflepuff' }]).map((x) => [x.key, x.crest + ' ' + x.name]));
    const houseSel = h('select', { class: 'select', 'aria-label': 'House' }, houseOpts.map(([v, t]) => h('option', { value: v, selected: v === st.house }, t)));
    const body = h('div', { class: 'card scorers-card' });
    page.appendChild(h('div', { class: 'toolbar scorers-toolbar' },
      segmented([['today', 'Today'], ['yesterday', 'Yesterday']], st.day, 'Day', (v) => { st.day = v; st.data = null; draw(); refresh(); }),
      houseSel, h('label', { class: 'search-box' }, icon('search'), search), h('span', { class: 'grow' }), live));
    page.appendChild(scorersLegend());
    page.appendChild(body);

    const setLive = () => {
      clear(live);
      if (st.error) { live.className = 'live-pill err'; append(live, [icon('alert'), 'Can’t update: ' + st.error]); return; }
      if (st.day === 'yesterday') { live.className = 'live-pill paused'; append(live, [icon('clock'), 'Yesterday, final']); return; }
      if (document.hidden) { live.className = 'live-pill paused'; append(live, [icon('pause'), 'Paused while hidden']); return; }
      live.className = 'live-pill';
      append(live, [h('span', { class: 'live-dot', 'aria-hidden': 'true' }), 'Live', st.updated ? h('span', { class: 'live-time' }, ' · updated ' + fmtTime.format(new Date(st.updated)) + ' IST') : null]);
    };
    const refresh = async () => {
      if (!page.isConnected) { clearInterval(st.timer); return; }
      if (st.loading || (document.hidden && st.data)) { setLive(); return; }
      st.loading = true;
      const day = st.day;
      try {
        const data = await api('GET', '/houses/scorers?day=' + day);
        if (day !== st.day || !page.isConnected) return;
        st.data = data; st.updated = Date.now(); st.error = null;
        draw();
      } catch (e) { st.error = e.message; }
      finally { st.loading = false; setLive(); }
    };
    const draw = () => {
      clear(body);
      if (!st.data) { body.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading today’s scorers…')); return; }
      const q = st.q.trim().toLowerCase();
      const rows = st.data.rows.filter((r) => (st.house === 'all' || r.house === st.house) && (!q || (r.name || '').toLowerCase().includes(q)));
      const head = h('div', { class: 'scorers-head' },
        h('b', null, plural(rows.length, 'member')),
        h('span', null, (st.day === 'today' ? 'Today, ' : 'Yesterday, ') + fmtDay(st.data.day) + ' · chat point at ' + st.data.chat_bar + ' messages, voice point at ' + st.data.voice_bar_min + ' minutes'),
        h('a', { class: 'open-link', href: '#/s/points' }, 'Limits', icon('right')));
      body.appendChild(head);
      if (!rows.length) { body.appendChild(h('div', { class: 'empty' }, icon('zap'), h('h3', null, q ? 'Nobody matches' : 'No scorers yet'), h('p', null, q ? 'Try another name.' : 'Points earned today show up here as they happen.'))); return; }
      const list = h('ol', { class: 'scorer-list' });
      rows.forEach((r) => {
        list.appendChild(h('li', { class: 'scorer-row', style: r.colour ? '--house:' + r.colour : '' },
          h('span', { class: 'sr-rank' }, r.rank),
          h('div', { class: 'sr-who' }, avatar(r.avatar, r.name || '?', 'lg'),
            h('div', { style: 'min-width:0' }, h('a', { class: 'sr-name', href: '#/members/' + r.id }, r.name || 'Former member'),
              h('span', { class: 'house-chip', style: '--house:' + (r.colour || 'var(--accent)') }, (r.crest || '') + ' ' + (r.house_name || r.house)))),
          h('div', { class: 'sr-total' }, h('b', null, fmtPoints(r.total)), h('small', null, r.total === 1 ? 'point' : 'points')),
          h('div', { class: 'sr-acts' }, r.activities.map(actChip))));
      });
      body.appendChild(list);
    };
    search.addEventListener('input', () => { st.q = search.value; draw(); });
    houseSel.addEventListener('change', () => { st.house = houseSel.value; draw(); });
    draw();
    clearInterval(st.timer);
    st.timer = setInterval(() => { if (st.day === 'today') refresh(); }, 20000);
    setLive();
    refresh();
    renderHouses.onVisible = () => { if (page.isConnected && !document.hidden && st.day === 'today') refresh(); else if (page.isConnected) setLive(); };
  }

  function fmtDay(ymd) {
    const d = new Date(ymd + 'T12:00:00+05:30');
    return isNaN(d) ? ymd : new Intl.DateTimeFormat('en-GB', { timeZone: IST, weekday: 'short', day: 'numeric', month: 'short' }).format(d);
  }

  // --- members ------------------------------------------------------------------------

  const TONE_ICONS = { normal: '🙂', gentle: '🤍', light_roast: '😏', roast: '🔥', respectful: '🎩', brief: '✂️' };

  function renderMembers(page) {
    document.title = 'Members · Loduchand';
    page.appendChild(pageHead('Members', 'Look anyone up: their points, activity, what the bot has seen from them, and the mods’ private notes it uses when it replies.'));
    const input = h('input', { type: 'search', placeholder: 'Search members by name', 'aria-label': 'Search members', autocomplete: 'off', spellcheck: 'false' });
    const results = h('ul', { class: 'member-results', 'aria-live': 'polite' });
    let timer = null, seq = 0;
    const run = async () => {
      const mine = ++seq;
      const q = input.value.trim();
      if (!q) { clear(results); results.hidden = true; return; }
      let list = [];
      try { list = await api('GET', '/members?q=' + encodeURIComponent(q)); } catch (e) { toast(e.message, 'error'); }
      if (mine !== seq) return;
      clear(results); results.hidden = false;
      if (!list.length) { results.appendChild(h('li', { class: 'empty-small', style: 'padding:14px 16px' }, 'Nobody called “' + q + '”.')); return; }
      list.forEach((m) => results.appendChild(h('li', null, h('a', { class: 'member-hit', href: '#/members/' + m.id },
        avatar(m.avatar, m.name, 'lg'), h('span', { class: 'grow' }, h('b', null, m.name), h('small', null, '@' + m.username)),
        m.has_note ? h('span', { class: 'badge src-panel' }, icon('edit'), 'Has notes') : null, m.bot ? h('span', { class: 'badge' }, 'Bot') : null, icon('right')))));
    };
    input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(run, 200); });
    input.addEventListener('keydown', (e) => { if (e.key === 'Enter') { const first = results.querySelector('a'); if (first) { e.preventDefault(); navigate(first.getAttribute('href')); } } });
    results.hidden = true;
    page.appendChild(h('div', { class: 'member-search' }, h('label', { class: 'search-box big' }, icon('search'), input), results));

    const notesCard = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading notes…'));
    page.appendChild(h('div', { class: 'section-title' }, h('h2', null, 'Members with notes'), h('span', null, 'Private to mods. Members never see these.')));
    page.appendChild(notesCard);
    api('GET', '/members/notes').then((list) => {
      clear(notesCard);
      if (!list.length) { notesCard.appendChild(h('div', { class: 'card empty' }, icon('edit'), h('h3', null, 'No notes yet'), h('p', null, 'Open a member and add a note to shape how the bot talks to them.'))); return; }
      const grid = h('div', { class: 'note-grid' });
      list.forEach((n) => grid.appendChild(h('a', { class: 'note-card' + (n.use_in_replies ? '' : ' is-off'), href: '#/members/' + n.user_id },
        h('div', { class: 'note-card-top' }, avatar(n.avatar, n.name, 'lg'), h('div', { class: 'grow' }, h('b', null, n.name), h('small', null, 'Edited ' + ago(n.updated_ts))),
          h('span', { class: 'badge tone-' + n.tone }, (TONE_ICONS[n.tone] || '') + ' ' + toneLabel(n.tone))),
        n.notes ? h('p', null, n.notes) : h('p', { class: 'muted' }, 'Tone only, no notes.'),
        n.use_in_replies ? null : h('span', { class: 'badge paused' }, 'Not used in replies'))));
      notesCard.appendChild(grid);
    }).catch((e) => { clear(notesCard).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); });
    requestAnimationFrame(() => input.focus({ preventScroll: true }));
  }

  function toneLabel(t) { return ({ normal: 'Normal', gentle: 'Gentle', light_roast: 'Light roast', roast: 'Roast', respectful: 'Respectful', brief: 'Brief' })[t] || t; }

  const profileState = { id: null, tab: 'overview', data: null };

  async function renderProfile(page, id, tab) {
    document.title = 'Member · Loduchand';
    profileState.tab = ['overview', 'seen', 'memories'].includes(tab) ? tab : 'overview';
    page.appendChild(h('a', { class: 'back-link', href: '#/members' }, icon('left'), 'Members'));
    const holder = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading the profile…'));
    page.appendChild(holder);
    let p;
    try { p = await api('GET', '/members/' + encodeURIComponent(id)); }
    catch (e) { clear(holder).appendChild(h('div', { class: 'card empty' }, icon('user'), h('h3', null, e.status === 404 ? 'No such member' : 'Can’t load this member'), h('p', null, e.message))); return; }
    if (!page.isConnected) return;
    profileState.id = id; profileState.data = p;
    document.title = p.name + ' · Members · Loduchand';
    clear(holder);

    // header
    const flags = [
      p.admin ? h('span', { class: 'badge who-admins' }, icon('shield'), 'Admin') : null,
      p.bot ? h('span', { class: 'badge' }, 'Bot') : null,
      p.captain ? h('span', { class: 'badge rank first' }, icon('trophy'), 'House captain') : null,
      p.muggle ? h('span', { class: 'badge paused' }, 'Muggle · out of the houses') : null,
      p.in_server ? null : h('span', { class: 'badge paused' }, 'Not in the server'),
    ];
    const hs = p.house;
    holder.appendChild(h('section', { class: 'profile-head card', style: hs ? '--house:' + hs.colour : '' },
      avatar(p.avatar, p.name, 'xl'),
      h('div', { class: 'grow' },
        h('div', { class: 'title-row' }, h('h1', null, p.name), hs ? h('span', { class: 'house-chip big' }, hs.crest + ' ' + hs.name) : null, flags),
        h('p', { class: 'profile-sub' }, p.username ? '@' + p.username : '', h('span', { class: 'field-key' }, p.id)),
        h('div', { class: 'profile-dates' },
          p.joined_at ? h('span', null, icon('calendar'), 'Joined ' + fmtDate(p.joined_at * 1000)) : null,
          p.created_at ? h('span', null, icon('user'), 'Account from ' + fmtDate(p.created_at * 1000)) : null,
          p.joins ? h('span', null, icon('repeat'), plural(p.joins.joins, 'join') + ', ' + plural(p.joins.leaves, 'leave')) : null),
        p.roles.length ? h('div', { class: 'chips role-chips' }, p.roles.map((r) => h('span', { class: 'chip role-chip' }, h('span', { class: 'role-dot', style: r.color ? 'background:' + r.color : '' }), h('span', { class: 'chip-text' }, r.name)))) : null)));

    const grid = h('div', { class: 'profile-grid' });
    const main = h('div', { class: 'profile-main' });
    const side = h('aside', { class: 'profile-side' });
    grid.appendChild(main); grid.appendChild(side);
    holder.appendChild(grid);
    side.appendChild(notesEditor(p));

    const panel = h('div', null);
    // Tabs swap the panel in place, so an unsaved note beside them survives.
    const show = (key) => {
      profileState.tab = key;
      tabs.querySelectorAll('a').forEach((a) => a.setAttribute('aria-current', a.dataset.tab === key ? 'page' : 'false'));
      const hash = '#/members/' + p.id + (key === 'overview' ? '' : '/' + key);
      if (location.hash !== hash) { history.replaceState(null, '', hash); currentHash = hash; }
      clear(panel);
      if (key === 'seen') profileSeen(panel, p);
      else if (key === 'memories') profileMemories(panel, p);
      else profileOverview(panel, p);
    };
    const tabs = h('nav', { class: 'page-tabs', 'aria-label': 'Profile sections' },
      [['overview', 'Overview', 'overview'], ['seen', 'What the bot sees', 'message'], ['memories', 'What it remembers', 'bot']].map(([key, label, ic]) =>
        h('a', { href: '#/members/' + p.id + (key === 'overview' ? '' : '/' + key), dataset: { tab: key }, onclick: (e) => { e.preventDefault(); show(key); } }, icon(ic), label)));
    main.appendChild(tabs);
    main.appendChild(panel);
    show(profileState.tab);
  }

  function fmtDate(ms) { return new Intl.DateTimeFormat('en-GB', { timeZone: IST, day: 'numeric', month: 'short', year: 'numeric' }).format(new Date(ms)); }

  function profileOverview(panel, p) {
    const pts = p.points;
    const today = card('pf-today', 'Today', pts.today_total ? fmtPoints(pts.today_total) + ' points so far, India time' : 'No points yet today, India time',
      h('div', { class: 'card-body pad' }, h('div', { class: 'sr-acts wide' }, pts.activities.map(actChip))));
    panel.appendChild(today);

    const month = pts.month.slice().sort((a, b) => b.points - a.points);
    const max = Math.max(1, ...month.map((m) => m.points));
    const bars = h('ul', { class: 'hbars' }, month.length ? month.map((m) => {
      const g = groupOf(m.source);
      return h('li', null, h('span', { class: 'hb-label' }, sourceLabel(m.source)),
        h('span', { class: 'hb-track' }, h('i', { style: 'width:' + Math.max(2, (Math.max(0, m.points) / max) * 100) + '%' })),
        h('b', null, fmtPoints(m.points)));
    }) : h('li', { class: 'empty-small' }, 'No points this month yet.'));
    panel.appendChild(card('pf-month', 'This month', null, h('div', { class: 'card-body pad' },
      h('div', { class: 'stat-row' },
        stat('Month', fmtPoints(pts.month_total), 'points'),
        stat('House rank', pts.house_rank ? '#' + pts.house_rank.rank : '—', pts.house_rank ? 'of ' + pts.house_rank.of + (p.house ? ' in ' + p.house.name : '') : 'not ranked'),
        stat('All time', fmtPoints(pts.all_time), 'points')),
      bars)));

    const a = p.activity;
    const hours = a.hours;
    const hmax = Math.max(1, ...hours);
    const peak = hours.indexOf(Math.max(...hours));
    const spark = h('div', { class: 'hours', role: 'img', 'aria-label': 'Messages by hour over 30 days; busiest around ' + String(peak).padStart(2, '0') + ':00 India time' },
      hours.map((n, i) => {
        const bar = h('span', { class: 'hour' + (i === peak && n ? ' peak' : ''), style: '--v:' + Math.max(n ? 6 : 2, Math.round((n / hmax) * 100)) + '%' });
        bar.addEventListener('mousemove', (e) => showTip(e, [String(i).padStart(2, '0') + ':00–' + String((i + 1) % 24).padStart(2, '0') + ':00 IST', plural(n, 'message') + ' in 30 days']));
        bar.addEventListener('mouseleave', hideTip);
        return bar;
      }));
    const chMax = Math.max(1, ...a.top_channels.map((c) => c.messages));
    panel.appendChild(card('pf-activity', 'Activity', 'Messages and voice, India time', h('div', { class: 'card-body pad' },
      h('div', { class: 'stat-row five' },
        stat('Today', numberFmt.format(a.messages_today), 'messages'),
        stat('7 days', numberFmt.format(a.messages_7d), 'messages'),
        stat('30 days', numberFmt.format(a.messages_30d), 'messages'),
        stat('🎙️ Voice', a.voice_today_min + ' min', 'today'),
        stat('🎙️ Voice', duration(a.voice_7d_min * 60), '7 days')),
      h('div', { class: 'two-mini' },
        h('div', null, h('h4', { class: 'mini-title' }, 'Busiest hours', h('span', null, hours.some(Boolean) ? 'peak ' + String(peak).padStart(2, '0') + ':00' : '')), spark,
          h('div', { class: 'range-scale' }, h('span', null, '00'), h('span', null, '06'), h('span', null, '12'), h('span', null, '18'), h('span', null, '23'))),
        h('div', null, h('h4', { class: 'mini-title' }, 'Top channels', h('span', null, '30 days')),
          a.top_channels.length ? h('ul', { class: 'hbars compact' }, a.top_channels.map((c) => h('li', null, h('span', { class: 'hb-label' }, '#' + (c.name || 'unknown')),
            h('span', { class: 'hb-track' }, h('i', { style: 'width:' + Math.max(2, (c.messages / chMax) * 100) + '%' })), h('b', null, numberFmt.format(c.messages))))) : h('p', { class: 'empty-small' }, 'No messages counted.'))))));

    const g = p.games;
    const rate = g.fights ? Math.round((g.wins / g.fights) * 100) + '% won' : 'no fights';
    panel.appendChild(card('pf-games', 'Games', null, h('div', { class: 'card-body pad' }, h('div', { class: 'stat-row four' },
      stat('🧠 Quiz', numberFmt.format(g.quiz_all), g.quiz_month + ' this month'),
      stat('⚔️ Fights', numberFmt.format(g.fights), g.wins + ' won (' + (g.fights ? Math.round((g.wins / g.fights) * 100) : 0) + '%)'),
      stat('👑 Crowns', numberFmt.format(g.crowns), 'royales won'),
      stat('🪽 Snitches', numberFmt.format(g.snitch_month), 'this month')))));

    if (p.joins) {
      const j = p.joins;
      const d = (s) => (s ? fmtDate(Date.parse(s)) : '—');
      panel.appendChild(card('pf-joins', 'Join history', 'From the member log', h('dl', { class: 'kv' },
        h('dt', null, 'Joins and leaves'), h('dd', null, plural(j.joins, 'join') + ', ' + plural(j.leaves, 'leave')),
        h('dt', null, 'First joined'), h('dd', null, d(j.first_join)),
        h('dt', null, 'Last joined'), h('dd', null, d(j.last_join)),
        h('dt', null, 'Last left'), h('dd', null, d(j.last_leave)))));
    }
  }

  function stat(label, value, sub) {
    return h('div', { class: 'stat' }, h('span', { class: 'stat-label' }, label), h('b', null, value), sub ? h('small', null, sub) : null);
  }
  function sourceLabel(key) {
    const labels = { chat: '💬 Chat', voice: '🎙️ Voice', quiz: '🧠 Quiz', koto: '🔤 Koto', anagram: '🔡 Anagram', cat: '🐱 Cat Bot', arena: '⚔️ Arena', royale: '👑 Battle Royale', snitch: '🪽 Snitch', golden_snitch: '🥇 Golden Snitch', weekly: '📝 Weekly posts', mod: '🛡️ Mods' };
    return labels[key] || key;
  }

  async function profileSeen(panel, p) {
    panel.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Reading the bot’s history…'));
    let data;
    try { data = await api('GET', '/members/' + p.id + '/seen'); } catch (e) { clear(panel).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); return; }
    clear(panel);
    const list = h('ul', { class: 'seen' });
    data.messages.forEach((m) => list.appendChild(h('li', null,
      h('div', { class: 'seen-meta' }, m.kind === 'chat' ? h('span', { class: 'badge src-panel' }, icon('message'), 'Talked to the bot') : h('span', { class: 'badge' }, icon('eye'), 'Read along'),
        m.channel ? h('span', { class: 'inline-ref' }, h('span', { class: 'glyph' }, '#'), m.channel) : null,
        h('span', { class: 'grow' }), h('small', { title: fmtFull.format(new Date(m.ts * 1000)) + ' IST' }, when(m.ts) + ' · ' + ago(m.ts))),
      h('p', { class: 'seen-text' }, m.text))));
    if (!data.messages.length) list.appendChild(h('li', { class: 'empty' }, icon('eye'), h('h3', null, 'Nothing stored'), h('p', null, 'The bot hasn’t kept any of their messages in the last ' + data.days + ' days.')));
    panel.appendChild(card('pf-seen', 'What the bot sees', 'Their latest messages in the bot’s conversation history · last ' + data.days + ' days, up to ' + data.limit, list));
  }

  async function profileMemories(panel, p) {
    panel.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Searching the bot’s memories…'));
    let data;
    try { data = await api('GET', '/members/' + p.id + '/memories'); } catch (e) { clear(panel).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); return; }
    clear(panel);
    const list = h('ul', { class: 'memories' });
    data.memories.forEach((m) => {
      const full = h('div', { class: 'memory-full', hidden: true }, markTerms(m.content + (m.truncated ? '\n…' : ''), m.matched));
      const toggle = h('button', { class: 'btn sm ghost', type: 'button', 'aria-expanded': 'false', onclick: () => { full.hidden = !full.hidden; toggle.setAttribute('aria-expanded', String(!full.hidden)); toggle.lastChild.textContent = full.hidden ? 'Read all' : 'Show less'; } }, icon('eye'), 'Read all');
      list.appendChild(h('li', null,
        h('div', { class: 'seen-meta' }, h('b', { class: 'memory-title' }, m.title), m.tags.map((t) => h('span', { class: 'badge' }, t)), h('span', { class: 'grow' }), h('small', null, fmtDate(m.ts * 1000))),
        h('p', { class: 'seen-text' }, markTerms(m.snippet, m.matched)),
        h('div', { class: 'memory-foot' }, h('small', null, 'Matched on ' + m.matched.join(', ')),
          m.truncated || m.snippet.startsWith('…') || m.snippet.endsWith('…') ? toggle : null), full));
    });
    if (!data.memories.length) list.appendChild(h('li', { class: 'empty' }, icon('bot'), h('h3', null, 'No memories mention them'), h('p', null, 'Searched ' + plural(data.searched, 'memory', 'memories') + ' for ' + data.terms.join(', ') + '.')));
    panel.appendChild(card('pf-memories', 'What the bot remembers', plural(data.memories.length, 'memory', 'memories') + ' naming them, out of ' + numberFmt.format(data.searched) + ' · read-only', list));
  }

  function markTerms(text, terms) {
    const words = (terms || []).filter(Boolean).map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    if (!words.length) return text;
    const re = new RegExp('(' + words.join('|') + ')', 'gi');
    return text.split(re).map((part, i) => (i % 2 ? h('mark', null, part) : part));
  }

  function notesEditor(p) {
    const saved = p.note;
    const d = { tone: saved ? saved.tone : 'normal', notes: saved ? saved.notes : '', use_in_replies: saved ? saved.use_in_replies : true };
    let base = JSON.stringify(d);
    const dirty = () => JSON.stringify(d) !== base;
    S.guard = () => (dirty() ? 1 : 0);
    const el = h('section', { class: 'card notes-card', 'aria-label': 'Mods’ notes' });
    const preview = h('pre', { class: 'ai-preview', 'aria-live': 'polite' });
    const previewNote = h('p', { class: 'hint', style: 'margin:6px 0 0' });
    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, saved ? 'Save note' : 'Add note');
    const status = h('span', { class: 'notes-status' });
    let ptimer = null, pseq = 0;
    const refreshPreview = () => {
      clearTimeout(ptimer);
      ptimer = setTimeout(async () => {
        const mine = ++pseq;
        try {
          const r = await api('POST', '/members/' + p.id + '/note/preview', d);
          if (mine !== pseq) return;
          preview.textContent = r.text || (d.use_in_replies ? 'Nothing yet: pick a tone or write a note.' : 'Nothing: this note is kept on the panel only.');
          preview.classList.toggle('empty-preview', !r.text);
          previewNote.textContent = r.switched_on ? 'Added before any message of theirs the bot answers, and messages that mention them.' : 'Member notes are switched off for the whole bot, so the AI gets none right now.';
          previewNote.className = r.switched_on ? 'hint' : 'error-text';
        } catch (_) { /* keep the last one */ }
      }, 220);
    };
    const updateState = () => {
      saveBtn.disabled = !dirty() || d.notes.length > p.max_note_chars;
      clear(status);
      if (dirty()) append(status, [h('span', { class: 'badge unsaved' }, h('span', { class: 'dot' }), 'Unsaved')]);
      else if (p.note) append(status, ['Edited ' + ago(p.note.updated_ts) + ' by ', memberName(p.note.updated_by)]);
    };
    const tones = h('div', { class: 'tones', role: 'radiogroup', 'aria-label': 'Tone' });
    p.tones.forEach((t) => {
      const b = h('button', { type: 'button', class: 'tone', role: 'radio', 'aria-checked': d.tone === t.value ? 'true' : 'false',
        onclick: () => { d.tone = t.value; tones.querySelectorAll('.tone').forEach((x) => x.setAttribute('aria-checked', x === b ? 'true' : 'false')); updateState(); refreshPreview(); } },
        h('span', { class: 'tone-icon', 'aria-hidden': 'true' }, TONE_ICONS[t.value] || ''), h('span', null, h('b', null, t.label), h('small', null, t.about)));
      tones.appendChild(b);
    });
    const ta = h('textarea', { class: 'textarea', id: 'note-text', rows: '6', maxlength: String(p.max_note_chars * 2), placeholder: 'Who they are, what they like, running jokes, things to avoid…' });
    ta.value = d.notes;
    const counter = h('span', { class: 'text-count' });
    const drawCount = () => { counter.textContent = d.notes.length + ' / ' + p.max_note_chars; counter.classList.toggle('over', d.notes.length > p.max_note_chars); };
    ta.addEventListener('input', () => { d.notes = ta.value; drawCount(); updateState(); refreshPreview(); });
    const sw = switchEl(d.use_in_replies, 'Use when replying', (on, btn) => { btn.set(on); d.use_in_replies = on; updateState(); refreshPreview(); });
    saveBtn.addEventListener('click', async () => {
      saveBtn.disabled = true;
      try {
        const r = await api('PUT', '/members/' + p.id + '/note', d);
        p.note = r.note; base = JSON.stringify(d);
        toast('Saved notes for ' + p.name);
        refreshAudit(); updateState();
        if (!del.isConnected) actions.insertBefore(del, actions.firstChild);
      } catch (e) { toast(e.message, 'error'); updateState(); }
    });
    const del = h('button', { class: 'btn danger', type: 'button', onclick: async () => {
      const ok = await confirmDialog({ title: 'Delete the notes for ' + p.name + '?', icon: 'trash', danger: true, body: 'The bot stops using them straight away. The change stays in the activity log.', confirm: 'Delete notes' });
      if (!ok) return;
      try {
        await api('DELETE', '/members/' + p.id + '/note');
        p.note = null; d.tone = 'normal'; d.notes = ''; d.use_in_replies = true; base = JSON.stringify(d);
        toast('Deleted the notes for ' + p.name); refreshAudit();
        const fresh = notesEditor(p); el.replaceWith(fresh);
      } catch (e) { toast(e.message, 'error'); }
    } }, icon('trash'), 'Delete');
    const actions = h('div', { class: 'notes-actions' }, saved ? del : null, h('span', { class: 'grow' }), saveBtn);
    append(el, [
      h('div', { class: 'card-head' }, h('div', { class: 'grow' }, h('h2', null, 'Mods’ notes'), h('div', { class: 'sub' }, icon('shield'), ' Private to mods. Never shown to members.'))),
      h('div', { class: 'card-body pad notes-body' },
        h('div', null, h('span', { class: 'label' }, 'How the bot treats them'), tones),
        h('div', null, h('div', { class: 'label-row' }, h('label', { class: 'label', for: 'note-text' }, 'Notes'), counter), ta),
        h('div', { class: 'label-row' }, h('span', null, h('b', { class: 'label', style: 'display:inline' }, 'Use when replying'), h('small', { class: 'hint', style: 'display:block;margin:0' }, 'Off keeps the note here only.')), sw),
        h('div', null, h('span', { class: 'label' }, 'What the AI gets'), preview, previewNote),
        status, actions),
    ]);
    drawCount(); updateState(); refreshPreview();
    return el;
  }

  function memberName(id) {
    const span = h('span', null, '…');
    memberById(id).then((m) => { span.textContent = m ? m.name : 'an admin'; });
    return span;
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
