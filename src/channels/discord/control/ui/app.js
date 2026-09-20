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
    spark: '<path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5 18 18M6 18l2.5-2.5M15.5 8.5 18 6"/>',
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
    upload: '<path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3"/>',
    image: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="m21 16-5-5-9 9"/>',
    send: '<path d="M21 3 10 14M21 3l-7 18-4-7-7-4z"/>',
    door: '<path d="M3 21h18"/><path d="M6 21V4.5A1.5 1.5 0 0 1 7.5 3h9A1.5 1.5 0 0 1 18 4.5V21"/><path d="M14.5 12.5h.01"/>',
    userminus: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M17 11h5"/>',
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
    welcomes: null, // special welcomes: { items, enabled, welcome_channel, ... }
    emojis: null,
    media: null, // the picture library, loaded when a page needs it
    templates: null,
    placeholders: null,
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
    S.welcomes = await api('GET', '/welcomes').catch(() => null);
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
      navItem('#/welcomes', icon('door'), 'Welcomes', S.welcomes && S.welcomes.active ? h('span', { class: 'nav-count', 'aria-label': plural(S.welcomes.active, 'welcome') + ' waiting', 'data-tip': 'Waiting for their member' }, String(S.welcomes.active)) : null),
      navItem('#/autoreplies', icon('reply'), 'Auto-responses', count(activeRules, S.rules.length)),
      navItem('#/members', icon('users'), 'Members', S.status && S.status.notes_to_review ? h('span', { class: 'nav-badge', 'aria-label': S.status.notes_to_review + ' notes to review', 'data-tip': 'Notes to review' }, S.status.notes_to_review) : null),
      navItem('#/messages', icon('message'), 'Messages'),
      navItem('#/deleted', icon('trash'), 'Deleted messages'),
      navItem('#/automod', icon('shield'), 'Moderation'),
      navItem('#/left', icon('userminus'), 'Left the server', S.status && S.status.left_recently ? h('span', { class: 'nav-count', 'aria-label': plural(S.status.left_recently, 'member') + ' left recently', 'data-tip': 'Left in the last ' + ((S.status && S.status.left_days) || 30) + ' days' }, String(S.status.left_recently)) : null),
      navItem('#/insights', icon('spark'), 'Insights'),
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
      ['Welcomes', '#/welcomes', 'door', 'A special message when a particular member joins'],
      ['Auto-responses', '#/autoreplies', 'reply', 'Answer or react to set words'],
      ['House Cup', '#/houses', 'trophy', 'Live house points, top scorers, latest points'],
      ['Bot behaviour', '#/agent', 'bot', 'Personality, tone, chattiness, model'],
      ['Members', '#/members', 'users', 'Profiles, what the bot sees, mods’ notes'],
      ['Messages', '#/messages', 'message', 'What a member has said, across every channel, newest first'],
      ['Search messages', '#/messages?q=', 'search', 'Find who said something and when, in every channel or in the long archive'],
      ['Deleted messages', '#/deleted', 'trash', 'Deleted and edited messages: what was said, who and when'],
      ['Edited messages', '#/deleted?tab=edited', 'edit', 'Messages members changed, before and after'],
      ['Left the server', '#/left', 'userminus', 'Members the bot has seen leave, and what they did while they were here'],
      ['Moderation', '#/automod', 'shield', 'Spam the bot removed, and messages it has asked a moderator to look at'],
      ['Possibly AI flags', '#/automod?kind=ai', 'bot', 'Messages that might have been written by an AI — flagged only, never deleted'],
      ['Insights', '#/insights', 'spark', 'Who replies to whom, duos, back-and-forths'],
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
    ((S.welcomes && S.welcomes.items) || []).forEach((w) => items.push({ kind: 'Welcomes', title: 'Welcome for ' + welcomeName(w), sub: w.lines[0] || '', href: '#/welcomes/' + w.id, lead: icon('door'), hay: welcomeName(w) + ' ' + w.user_id + ' ' + w.lines.join(' ') + ' ' + (w.note || '') }));
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
      const order = ['Pages', 'Features', 'Settings', 'Commands', 'Reminders', 'Welcomes', 'Auto-responses'];
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

  function dirtyCount() {
    return S.fields.filter((f) => f.dirty()).length + (S.guard ? S.guard() : 0) + (S.guards || []).reduce((n, g) => n + g.count(), 0);
  }

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
    S.guards = [];
    S.discard = null;
    if (agentBar) { agentBar.remove(); agentBar = null; }
    renderSidebar();
    const page = clear(shell.page);
    removeSavebar();
    switch (r.name) {
      case 'section': renderSection(page, r.parts[1], r.q.get('k')); break;
      case 'reminders': if (r.parts[1] === 'members') renderMemos(page); else { renderReminders(page); if (r.parts[1]) openReminderEditor(r.parts[1], r.q); } break;
      case 'welcomes': renderWelcomes(page); if (r.parts[1]) openWelcomeEditor(r.parts[1]); break;
      case 'autoreplies': renderRules(page); if (r.parts[1]) openRuleEditor(r.parts[1]); break;
      case 'houses': if (r.parts[1] === 'scorers') renderScorers(page); else renderHouses(page); break;
      case 'members': if (r.parts[1]) renderProfile(page, r.parts[1], r.parts[2]); else renderMembers(page); break;
      case 'agent': renderAgent(page); break;
      case 'insights': renderInsights(page); break;
      case 'commands': renderCommands(page, r.q.get('q') || ''); break;
      case 'activity': renderActivity(page, r.q); break;
      case 'messages': case 'search': renderMessages(page, r.q); break;
      case 'deleted': renderDeleted(page, r.q); break;
      case 'left': renderLeft(page, r.q); break;
      case 'automod': renderAutomod(page, r.q); break;
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
      tile('Members', 'users', guild ? numberFmt.format(guild.members) : '—', st.left_recently
        ? h('a', { class: 'tile-link', href: '#/left' }, icon('userminus'), st.left_recently + ' left in the last ' + (st.left_days || 30) + ' days')
        : (guild ? guild.name : 'Server not loaded yet')),
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
      case 'times': return wrap(value.split(',').filter(Boolean).join(', '));
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
    if (sec.id === 'arena') renderArena(page);
    if (sec.id === 'frogs') renderFrogs(page);
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
      case 'times': return v.split(',').filter(Boolean).join(', ');
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
    if (k.type === 'times' && !v.split(',').filter(Boolean).length) return 'Add at least one time.';
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
      case 'times': {
        const list = f.draft.split(',').map((t) => t.trim()).filter(Boolean);
        const times = h('div', { class: 'times' });
        list.forEach((t) => times.appendChild(h('span', { class: 'chip' }, icon('clock'), h('span', { class: 'chip-text mono' }, t),
          h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + t, onclick: () => set(list.filter((x) => x !== t).join(','), true) }, icon('x')))));
        if (!list.length) times.appendChild(h('span', { class: 'hint', style: 'margin:0' }, 'No times yet.'));
        const tIn = h('input', { class: 'input', type: 'time', id, 'aria-label': 'Time to add', 'aria-describedby': described });
        const add = () => {
          if (parseHm(tIn.value) === null) { tIn.focus(); return; }
          const v = hm(parseHm(tIn.value));
          if (!list.includes(v)) { list.push(v); list.sort(); }
          set(list.join(','), true);
          const again = f.el.querySelector('.field-control input[type=time]');
          if (again) again.focus();
        };
        tIn.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); add(); } });
        return h('div', null, times, h('div', { class: 'time-add', style: 'margin-top:8px' }, h('div', { class: 'input-group', style: 'max-width:220px' }, tIn, h('span', { class: 'addon' }, 'IST')), h('button', { class: 'btn sm', type: 'button', onclick: add }, icon('plus'), 'Add time')));
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
      secSel.appendChild(h('option', { value: 'welcomes', selected: section === 'welcomes' }, '👋  Welcomes'));
      if (!sectionById('autoreplies')) secSel.appendChild(h('option', { value: 'autoreplies', selected: section === 'autoreplies' }, '💬  Auto-responses'));
      secSel.appendChild(h('option', { value: 'agent', selected: section === 'agent' }, '🤖  Bot behaviour'));
      secSel.appendChild(h('option', { value: 'messages', selected: section === 'messages' }, '💬  Messages'));
      secSel.appendChild(h('option', { value: 'left', selected: section === 'left' }, '🚪  Left the server'));
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

  // --- search messages ----------------------------------------------------------------

  const SEARCH_PERIODS = [['1', '1 day'], ['7', '7 days'], ['30', '30 days'], ['90', '90 days'], ['all', 'All']];

  /** "15 Sep, 9:41 pm", India time; the year too when it isn't this one. */
  function msgWhen(ts) {
    const d = new Date(ts * 1000);
    const parts = new Intl.DateTimeFormat('en-US', { timeZone: IST, year: 'numeric', hour: 'numeric', minute: '2-digit', hour12: true }).formatToParts(d);
    const get = (t) => (parts.find((x) => x.type === t) || {}).value || '';
    const thisYear = new Intl.DateTimeFormat('en-US', { timeZone: IST, year: 'numeric' }).format(new Date());
    return dayMonth(ts) + (get('year') !== thisYear ? ' ' + get('year') : '') + ', ' + get('hour') + ':' + get('minute') + ' ' + get('dayPeriod').toLowerCase();
  }

  /** The text with characters start..end (code points, as the server counts) marked. */
  function markRange(text, start, end) {
    const cp = Array.from(text || '');
    if (!(end > start) || start >= cp.length) return text;
    return [cp.slice(0, start).join(''), h('mark', null, cp.slice(start, end).join('')), cp.slice(end).join('')];
  }

  function searchChannelItems(selected) {
    return (q) => {
      q = q.toLowerCase();
      const list = S.channels.filter((c) => (c.kind === 'text' || c.kind === 'voice') && !/safe-corner/i.test(c.name)
        && (!q || c.name.toLowerCase().includes(q) || (c.category || '').toLowerCase().includes(q)))
        .map((c) => ({ id: c.id, label: c.name, group: c.category || '', lead: c.kind === 'voice' ? icon('voice') : h('span', { class: 'glyph' }, '#'),
          trail: selected === c.id ? icon('check', 'check-mark') : null }));
      return (q ? [] : [{ id: '', label: 'All channels', lead: icon('hash'), trail: !selected ? icon('check', 'check-mark') : null }]).concat(list);
    };
  }

  /**
   * Messages: what the server has said, member first.
   *
   * Two stores hold message text and they cover different things, so the page
   * reads one at a time and says which. "Every channel" is the bot's own copy of
   * every channel it can see, kept for a year. "Long archive" is the AI's stored
   * history: back to the bot's first day, but only the channels it chats in, so
   * a house channel is simply not in it. Mixing them would make a silence mean
   * two things at once.
   */
  const MSG_SOURCES = [['log', 'Every channel'], ['archive', 'Long archive']];

  function renderMessages(page, rq) {
    document.title = 'Messages · Loduchand';
    page.appendChild(pageHead('Messages', 'What the server has said. Pick a member to read theirs, newest first, across every channel — then search inside it.'));
    const days = rq.get('days');
    const src = rq.get('source');
    const st = {
      q: (rq.get('q') || '').trim().slice(0, 100),
      member: /^\d{1,20}$/.test(rq.get('member') || '') ? rq.get('member') : '',
      channel: /^\d{1,20}$/.test(rq.get('channel') || '') ? rq.get('channel') : '',
      days: SEARCH_PERIODS.some((x) => x[0] === days) ? days : '30',
      source: MSG_SOURCES.some((x) => x[0] === src) ? src : 'log',
    };
    const hashFor = (over) => {
      const s = Object.assign({}, st, over || {});
      const p = new URLSearchParams();
      if (s.q) p.set('q', s.q);
      if (s.member) p.set('member', s.member);
      if (s.channel) p.set('channel', s.channel);
      if (s.days !== '30') p.set('days', s.days);
      if (s.source !== 'log') p.set('source', s.source);
      const qs = p.toString();
      return '#/messages' + (qs ? '?' + qs : '');
    };
    const apply = (over) => navigate(hashFor(over));

    // --- the filters ------------------------------------------------------------------
    const input = h('input', { type: 'search', value: st.q, placeholder: st.member ? 'Search within their messages' : 'Search every message, e.g. koto',
      'aria-label': 'Words to find', maxlength: '100', autocomplete: 'off', spellcheck: 'false', enterkeyhint: 'search' });
    const tooShort = h('p', { class: 'hint msg-short', hidden: true, role: 'status' }, 'Type at least 2 characters, or leave it empty to see everything.');
    const go = () => {
      const typed = input.value.trim();
      if (typed && Array.from(typed).length < 2) { tooShort.hidden = false; input.focus(); return; }
      tooShort.hidden = true;
      apply({ q: typed });
    };
    input.addEventListener('search', () => { if (!input.value.trim() && st.q) apply({ q: '' }); });

    const memberBtn = h('button', { class: 'picker-btn wide', type: 'button', 'aria-haspopup': 'listbox', 'aria-label': 'Member' });
    const drawMember = (m) => {
      clear(memberBtn);
      append(memberBtn, [st.member && m ? avatar(m.avatar, m.name, 'xs') : icon(st.member ? 'user' : 'users'),
        h('span', { class: 'value' + (st.member ? '' : ' placeholder') }, st.member ? (m ? m.name : 'Member ' + st.member) : 'Everyone'), icon('chevron')]);
    };
    drawMember(S.members.get(st.member) || null);
    if (st.member) memberById(st.member).then((m) => { if (memberBtn.isConnected) drawMember(m); });
    memberBtn.addEventListener('click', () => openPicker(memberBtn, { title: 'Whose messages', placeholder: 'Search members by name', debounce: 180,
      load: async (q) => (q ? [] : [{ id: '', label: 'Everyone', lead: icon('users'), trail: !st.member ? icon('check', 'check-mark') : null }]).concat(await memberItems(q)),
      onPick: (it) => apply({ member: it.id }) }));

    const channelBtn = h('button', { class: 'picker-btn', type: 'button', 'aria-haspopup': 'listbox', 'aria-label': 'Channel' });
    const c = st.channel ? chan(st.channel) : null;
    append(channelBtn, [c && c.kind === 'voice' ? icon('voice') : h('span', { class: 'glyph' }, '#'),
      h('span', { class: 'value' + (st.channel ? '' : ' placeholder') }, st.channel ? (c ? c.name : 'channel ' + st.channel) : 'All channels'), icon('chevron')]);
    channelBtn.addEventListener('click', () => openPicker(channelBtn, { title: 'Channel', placeholder: 'Search channels',
      load: searchChannelItems(st.channel), onPick: (it) => apply({ channel: it.id }) }));

    const period = segmented(SEARCH_PERIODS, st.days, 'Period', (v) => apply({ days: v }));
    period.classList.add('msg-periods');
    const source = segmented(MSG_SOURCES, st.source, 'Where to look', (v) => apply({ source: v }));
    source.classList.add('msg-sources');
    const coverNote = h('p', { class: 'msg-cover' });

    const form = h('form', { class: 'card msg-search messages-filters', role: 'search', onsubmit: (e) => { e.preventDefault(); go(); } },
      h('div', { class: 'msg-search-row' }, memberBtn, h('label', { class: 'search-box big' }, icon('search'), input),
        h('button', { class: 'btn primary', type: 'submit' }, 'Search')),
      tooShort,
      h('div', { class: 'msg-search-filters' }, channelBtn, period, h('span', { class: 'grow' }), source),
      coverNote);
    page.appendChild(form);

    const out = h('div', { class: 'msg-out' });
    page.appendChild(out);

    // --- the list ---------------------------------------------------------------------
    let items = [], next = null, last = null, busy = false, failed = null, failedStatus = 0;
    const summary = h('div', { class: 'msg-summary', 'aria-live': 'polite' });
    const list = h('div', { class: 'msg-list' });
    const foot = h('div', { class: 'msg-foot' });
    const box = h('div', { class: 'card msg-results' }, list, foot);
    out.appendChild(summary);
    out.appendChild(box);

    const memberName = () => { const m = S.members.get(st.member); return m ? m.name : 'that member'; };
    const channelName = () => { const x = chan(st.channel); return x ? x.name : 'that channel'; };
    const periodWords = () => st.days === 'all' ? 'everything kept here' : st.days === '1' ? 'the last day' : 'the last ' + st.days + ' days';
    const inPeriod = () => st.days === 'all' ? 'anything kept here' : 'anything in ' + periodWords();
    const whoWords = () => {
      const bits = [];
      if (st.member) bits.push('from ' + memberName());
      if (st.channel) bits.push('in #' + channelName());
      return bits.length ? ' ' + bits.join(' ') : '';
    };

    /** What this store can see, said plainly, so a silence is never mistaken for proof. */
    const drawCover = () => {
      clear(coverNote);
      const cov = (last && last.coverage) || {};
      if (st.source === 'archive') {
        append(coverNote, [icon('info'), cov.channels
          ? h('span', null, 'The AI’s stored history: back to the bot’s first day, but only the ',
              h('b', null, cov.channels + ' channels'), ' it is allowed to chat in. A house or staff channel was never in it.')
          : h('span', null, 'The AI’s stored history: back to the bot’s first day, across every channel it can chat in.')]);
        return;
      }
      const back = cov.oldest_ts ? 'back to ' + dayMonth(cov.oldest_ts) : 'from the day this was switched on';
      const bits = ['Every channel the bot can see except #safe-corner, ' + back + '.'];
      if (cov.text_days) bits.push('Kept for ' + cov.text_days + ' days.');
      if (cov.skipped) bits.push(plural(cov.skipped, 'channel') + ' on the skip list.');
      append(coverNote, [icon('info'), h('span', null, bits.join(' '))]);
      if (cov.enabled === false) coverNote.appendChild(h('b', { class: 'msg-cover-off' }, ' Recording is off, so nothing new is being kept.'));
    };

    const otherSource = () => h('button', { class: 'btn sm', type: 'button', onclick: () => apply({ source: st.source === 'log' ? 'archive' : 'log' }) },
      icon(st.source === 'log' ? 'clock' : 'hash'),
      st.source === 'log' ? 'Look further back in the long archive' : 'Look in every channel instead');

    const moreBtn = () => h('button', { class: 'btn', type: 'button', disabled: busy, onclick: () => load() },
      busy ? h('span', { class: 'spinner' }) : icon('down'), items.length ? 'Load more' : 'Keep looking further back');

    const draw = () => {
      clear(summary); clear(list); clear(foot);
      drawCover();
      box.classList.toggle('is-empty', !items.length);
      if (!items.length && busy) {
        list.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Reading…'));
        foot.hidden = true;
        return;
      }
      if (failed && !items.length) {
        list.appendChild(h('div', { class: 'empty' }, icon('alert'), h('h3', null, 'Couldn’t read the messages'), h('p', null, failed),
          failedStatus === 0 || failedStatus >= 500 ? h('p', { style: 'margin-top:12px' }, h('button', { class: 'btn', type: 'button', onclick: () => load() }, icon('restart'), 'Try again')) : null));
        foot.hidden = true;
        return;
      }
      const shownQ = st.q ? '“' + st.q + '”' : '';
      const count = h('b', null, numberFmt.format(items.length) + (items.length === 1 ? ' message' : ' messages'));
      summary.appendChild(st.q
        ? h('span', null, count, ' found' + (next ? ' so far' : ''), ' for ', h('b', null, shownQ), whoWords())
        : h('span', null, next ? 'The newest ' : 'All ', count, st.member ? ' from ' : ' across the server', st.member ? h('b', null, memberName()) : null,
            st.channel ? ' in #' + channelName() : '', next ? '' : ', newest first'));
      if (!items.length) {
        if (next) {
          list.appendChild(h('div', { class: 'empty' }, icon('search'), h('h3', null, 'Nothing yet in what has been read'),
            h('p', null, 'The read pauses after a long stretch so the bot stays quick. It can keep looking further back.')));
        } else {
          list.appendChild(h('div', { class: 'empty' }, icon('search'),
            h('h3', null, st.q ? 'No messages found' : 'Nothing kept here yet'),
            h('p', null, st.q ? 'Nothing ' + (st.days === 'all' ? 'kept here' : 'in ' + periodWords()) + ' matches ' + shownQ + whoWords() + '.'
              : 'There is no ' + inPeriod() + whoWords() + '.'),
            h('p', { class: 'hint' }, st.source === 'log'
              ? 'The long archive goes back further, but only covers the channels the bot chats in.'
              : 'The archive only covers the channels the bot chats in. Every other channel is in the bot’s own copy.'),
            h('p', { style: 'margin-top:12px' }, otherSource())));
        }
      }
      items.forEach((r) => list.appendChild(msgHit(r)));
      // With nothing to show, the empty state has already said everything the foot would.
      foot.hidden = !items.length && !next;
      if (foot.hidden) return;
      if (failed) foot.appendChild(h('span', { class: 'msg-foot-error' }, icon('alert'), failed));
      const reached = last && (last.scanned_to || (items.length && items[items.length - 1].ts));
      if (next) foot.appendChild(h('span', null, reached ? 'Read back to ' + dayMonth(reached) : 'There is more below'));
      else foot.appendChild(h('span', null, st.days === 'all' ? 'That is everything this store has' : 'That is all of ' + periodWords()));
      if (next) foot.appendChild(moreBtn());
      else if (items.length) foot.appendChild(otherSource());
    };

    const load = async () => {
      if (busy) return;
      busy = true; failed = null;
      if (items.length) { const b = foot.querySelector('.btn'); if (b) { b.disabled = true; b.replaceChild(h('span', { class: 'spinner' }), b.firstChild); } } else draw();
      const p = new URLSearchParams({ days: st.days });
      if (st.q) p.set('q', st.q);
      if (st.member) p.set('member', st.member);
      if (st.channel) p.set('channel', st.channel);
      if (next) p.set('before', String(next));
      const path = st.source === 'archive' ? '/messages/search?' : '/messages?';
      try {
        const data = await api('GET', path + p.toString());
        if (!box.isConnected) return;
        if (data.member && data.member.name && !S.members.has(data.member.id)) S.members.set(data.member.id, { id: data.member.id, name: data.member.name });
        items = items.concat(data.results);
        next = data.next_before;
        last = data;
      } catch (e) {
        if (!box.isConnected) return;
        failed = e.message;
        failedStatus = e.status || 0;
      }
      busy = false;
      draw();
      refreshAudit();
    };
    // The archive can only be searched, never browsed: it needs words to start.
    if (st.source === 'archive' && Array.from(st.q).length < 2) {
      drawCover();
      clear(list); clear(foot);
      box.classList.add('is-empty');
      foot.hidden = true;
      list.appendChild(h('div', { class: 'empty msg-empty' }, icon('search'), h('h3', null, 'Search the long archive'),
        h('p', null, 'The archive is searched by words, not browsed. Type a word or phrase and press Enter.'),
        h('p', { class: 'hint' }, 'To read someone’s messages from the beginning instead, look in every channel.'),
        h('p', { style: 'margin-top:12px' }, otherSource())));
      setTimeout(() => { if (input.isConnected) input.focus(); }, 0);
      return;
    }
    load();
  }

  function msgHit(r) {
    const m = r.member || {};
    const name = m.name || 'Member ' + m.id;
    const long = Array.from(r.text).length > 420;
    const textEl = h('p', { class: 'msg-hit-text' });
    const fill = (full) => { clear(textEl); append(textEl, full ? markRange(r.text, r.match[0], r.match[1]) : markRange(r.snippet.text, r.snippet.start, r.snippet.end)); };
    fill(!long);
    const more = long ? h('button', { class: 'btn sm ghost msg-hit-more', type: 'button', 'aria-expanded': 'false', onclick: () => {
      const open = more.getAttribute('aria-expanded') !== 'true';
      fill(open);
      more.setAttribute('aria-expanded', String(open));
      more.lastChild.textContent = open ? 'Show less' : 'Show the whole message';
    } }, icon('eye'), 'Show the whole message') : null;
    const ch = r.channel || {};
    const d = new Date(r.ts * 1000);
    return h('article', { class: 'msg-hit' },
      h('a', { class: 'msg-hit-avatar', href: '#/members/' + m.id, tabindex: '-1', 'aria-hidden': 'true' }, avatar(m.avatar, name, 'lg')),
      h('div', { class: 'msg-hit-main' },
        h('div', { class: 'msg-hit-head' },
          h('a', { class: 'msg-hit-name', href: '#/members/' + m.id }, name),
          r.house ? h('span', { class: 'house-chip', style: '--house:' + r.house.colour }, r.house.crest + ' ' + r.house.name) : null,
          h('span', { class: 'msg-hit-chan', title: ch.thread ? 'A thread in #' + ch.name : null }, h('span', { class: 'glyph' }, '#'), ch.name, ch.thread ? h('small', null, '› thread') : null),
          h('time', { class: 'msg-hit-time', datetime: d.toISOString(), title: fmtFull.format(d) + ' IST' }, msgWhen(r.ts), h('small', null, ' · ' + ago(r.ts)))),
        r.reply_to ? h('p', { class: 'msg-hit-reply' }, icon('reply'), h('span', null, 'replying to ', h('b', null, '@' + (r.reply_to.author || 'someone')), r.reply_to.text ? ': “' + r.reply_to.text + '”' : '')) : null,
        textEl, more,
        (r.files || []).length ? h('p', { class: 'msg-hit-files' }, icon(r.files.every((f) => f.image) ? 'image' : 'tag'),
          r.files.map((f) => f.name).join(', ')) : null),
      h('div', { class: 'msg-hit-actions' }, r.url
        ? h('a', { class: 'btn sm', href: r.url, target: '_blank', rel: 'noopener', 'aria-label': 'Open ' + name + '’s message in Discord' }, 'Open in Discord', icon('external'))
        : h('span', { class: 'hint', title: 'The bot didn’t keep this message’s id, so there’s no link.' }, 'No link')));
  }

  // --- deleted & edited messages ------------------------------------------------------

  const LOG_PERIODS = [['1', '1 day'], ['7', '7 days'], ['30', '30 days']];

  /** "9:43 pm", India time. */
  function msgClock(ts) {
    const parts = new Intl.DateTimeFormat('en-US', { timeZone: IST, hour: 'numeric', minute: '2-digit', hour12: true }).formatToParts(new Date(ts * 1000));
    const get = (t) => (parts.find((x) => x.type === t) || {}).value || '';
    return get('hour') + ':' + get('minute') + ' ' + get('dayPeriod').toLowerCase();
  }
  function sameIstDay(a, b) { const f = (ts) => istParts(ts * 1000, { year: 'numeric', month: 'numeric', day: 'numeric' }); return f(a) === f(b); }
  function laterWords(secs) {
    if (secs < 60) return 'under a minute later';
    if (secs < 3600) return Math.round(secs / 60) + ' min later';
    if (secs < 86400) { const n = Math.round(secs / 3600); return n + (n === 1 ? ' hour later' : ' hours later'); }
    const d = Math.round(secs / 86400);
    return d + (d === 1 ? ' day later' : ' days later');
  }
  function sizeWords(bytes) {
    if (bytes >= 1024 * 1024) return (bytes / 1024 / 1024).toFixed(bytes >= 10 * 1024 * 1024 ? 0 : 1) + ' MB';
    return Math.max(1, Math.round(bytes / 1024)) + ' KB';
  }

  /** A picture over the page, with the others from the same message a key away. */
  function openLightbox(images, start) {
    const before = document.activeElement;
    let i = start, layer = null;
    const img = h('img', { class: 'lightbox-img', alt: '' });
    const name = h('span', { class: 'lightbox-name' });
    const count = h('span', { class: 'lightbox-count' });
    const full = h('a', { class: 'btn sm', target: '_blank', rel: 'noopener' }, 'Open in a new tab', icon('external'));
    const show = () => {
      img.src = images[i].url; img.alt = images[i].name || 'Picture';
      name.textContent = images[i].name || 'Picture';
      count.textContent = images.length > 1 ? (i + 1) + ' of ' + images.length : '';
      full.href = images[i].url;
    };
    const step = (d) => { i = (i + d + images.length) % images.length; show(); };
    const close = () => { wrap.remove(); document.removeEventListener('keydown', keys); popLayer(layer); if (before && before.focus) before.focus(); };
    const keys = (e) => { if (images.length > 1 && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) { step(e.key === 'ArrowLeft' ? -1 : 1); e.preventDefault(); } };
    const closeBtn = h('button', { class: 'btn sm ghost icon-only lightbox-close', type: 'button', 'aria-label': 'Close', onclick: close }, icon('x'));
    const nav = images.length > 1 ? [
      h('button', { class: 'lightbox-nav prev', type: 'button', 'aria-label': 'Previous picture', onclick: () => step(-1) }, icon('left')),
      h('button', { class: 'lightbox-nav next', type: 'button', 'aria-label': 'Next picture', onclick: () => step(1) }, icon('right')),
    ] : null;
    const wrap = h('div', { class: 'lightbox', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Picture from a deleted message' },
      h('div', { class: 'scrim', onclick: close }),
      h('figure', { class: 'lightbox-frame' }, img, nav,
        h('figcaption', { class: 'lightbox-foot' }, h('span', { class: 'grow lightbox-title' }, name, count), full, closeBtn)));
    wrap.addEventListener('keydown', (e) => { if (e.key === 'Tab') trapFocus(wrap, e); });
    document.addEventListener('keydown', keys);
    layer = pushLayer({ close });
    show();
    $('#layers').appendChild(wrap);
    closeBtn.focus();
  }

  function renderDeleted(page, rq) {
    const tab = rq.get('tab') === 'edited' ? 'edited' : 'deleted';
    document.title = (tab === 'edited' ? 'Edited' : 'Deleted') + ' messages · Loduchand';
    const days = rq.get('days');
    const st = {
      tab,
      q: (rq.get('q') || '').trim().slice(0, 100),
      member: /^\d{1,20}$/.test(rq.get('member') || '') ? rq.get('member') : '',
      channel: /^\d{1,20}$/.test(rq.get('channel') || '') ? rq.get('channel') : '',
      days: LOG_PERIODS.some((x) => x[0] === days) ? days : '30',
    };
    const hashFor = (over) => {
      const s = Object.assign({}, st, over || {});
      const p = new URLSearchParams();
      if (s.tab === 'edited') p.set('tab', 'edited');
      if (s.member) p.set('member', s.member);
      if (s.channel) p.set('channel', s.channel);
      if (s.days !== '30') p.set('days', s.days);
      if (s.q) p.set('q', s.q);
      const qs = p.toString();
      return '#/deleted' + (qs ? '?' + qs : '');
    };
    const setting = (key, fallback) => {
      const sec = sectionById('msglog');
      const found = sec && sec.settings.find((x) => x.key === key);
      const v = found && (found.value || found.default);
      return v && /^\d+$/.test(v) ? +v : fallback;
    };
    const logDays = setting('VIZIER_MSGLOG_LOG_DAYS', 30);

    page.appendChild(pageHead('Deleted messages', 'What members deleted or changed, and when. Shown only here, never posted in Discord.',
      sectionById('msglog') ? h('a', { class: 'btn', href: '#/s/msglog', 'aria-label': 'Deleted messages settings' }, icon('sliders'), h('span', { class: 'hide-sm' }, 'Settings')) : null));
    page.appendChild(h('nav', { class: 'page-tabs', 'aria-label': 'Message log views' },
      h('a', { href: hashFor({ tab: 'deleted' }), 'aria-current': tab === 'deleted' ? 'page' : null }, icon('trash'), 'Deleted'),
      h('a', { href: hashFor({ tab: 'edited' }), 'aria-current': tab === 'edited' ? 'page' : null }, icon('edit'), 'Edited')));
    const note = h('div', { class: 'banner info inline msglog-note', role: 'note' }, icon('info'),
      h('p', null, 'Kept ' + plural(logDays, 'day') + '. ', h('span', null, 'Discord doesn’t tell bots who deleted a message. Never includes #safe-corner or DMs.')));
    page.appendChild(note);

    // Filters: each applies at once and lives in the address.
    const input = h('input', { type: 'search', value: st.q, placeholder: tab === 'edited' ? 'Words in the old or new text' : 'Words in the deleted text', 'aria-label': 'Filter by text', maxlength: '100', autocomplete: 'off', spellcheck: 'false', enterkeyhint: 'search' });
    const apply = (over) => navigate(hashFor(over));
    input.addEventListener('search', () => { if (!input.value.trim() && st.q) apply({ q: '' }); });

    const memberBtn = h('button', { class: 'picker-btn', type: 'button', 'aria-haspopup': 'listbox', 'aria-label': 'Member' });
    const drawMember = (m) => {
      clear(memberBtn);
      append(memberBtn, [st.member && m ? avatar(m.avatar, m.name, 'xs') : icon(st.member ? 'user' : 'users'),
        h('span', { class: 'value' + (st.member ? '' : ' placeholder') }, st.member ? (m ? m.name : 'Member ' + st.member) : 'Any member'), icon('chevron')]);
    };
    drawMember(null);
    if (st.member) memberById(st.member).then((m) => { if (memberBtn.isConnected) drawMember(m); });
    memberBtn.addEventListener('click', () => openPicker(memberBtn, { title: 'Member', placeholder: 'Search members by name', debounce: 180,
      load: async (q) => (q ? [] : [{ id: '', label: 'Any member', lead: icon('users'), trail: !st.member ? icon('check', 'check-mark') : null }]).concat(await memberItems(q)),
      onPick: (it) => apply({ member: it.id }) }));

    const channelBtn = h('button', { class: 'picker-btn', type: 'button', 'aria-haspopup': 'listbox', 'aria-label': 'Channel' });
    const c0 = st.channel ? chan(st.channel) : null;
    append(channelBtn, [c0 && c0.kind === 'voice' ? icon('voice') : h('span', { class: 'glyph' }, '#'),
      h('span', { class: 'value' + (st.channel ? '' : ' placeholder') }, st.channel ? (c0 ? c0.name : 'channel ' + st.channel) : 'All channels'), icon('chevron')]);
    channelBtn.addEventListener('click', () => openPicker(channelBtn, { title: 'Channel', placeholder: 'Search channels', load: searchChannelItems(st.channel), onPick: (it) => apply({ channel: it.id }) }));

    const period = segmented(LOG_PERIODS, st.days, 'Period', (v) => apply({ days: v }));
    const filters = h('form', { class: 'card msg-search msglog-filters', role: 'search', onsubmit: (e) => { e.preventDefault(); apply({ q: input.value.trim() }); } },
      h('div', { class: 'msg-search-row' }, h('label', { class: 'search-box' }, icon('search'), input)),
      h('div', { class: 'msg-search-filters' }, memberBtn, channelBtn, period));
    page.appendChild(filters);

    const summary = h('div', { class: 'msg-summary', 'aria-live': 'polite' });
    const list = h('div', { class: 'msg-list' });
    const foot = h('div', { class: 'msg-foot' });
    const box = h('div', { class: 'card msg-results msglog-results' }, list, foot);
    page.appendChild(summary);
    page.appendChild(box);

    let items = [], next = null, last = null, busy = false, failed = null, failedStatus = 0;
    const periodWords = () => st.days === '1' ? 'the last day' : 'the last ' + st.days + ' days';
    const filterWords = () => {
      const bits = [];
      if (st.member) { const m = S.members.get(st.member); bits.push('from ' + (m ? m.name : 'that member')); }
      if (st.channel) { const c = chan(st.channel); bits.push('in #' + (c ? c.name : 'that channel')); }
      if (st.q) bits.push('with “' + st.q + '”');
      return bits.length ? ' ' + bits.join(' ') : '';
    };
    const noun = tab === 'edited' ? ['edit', 'edits'] : ['deleted message', 'deleted messages'];
    const draw = () => {
      clear(summary); clear(list); clear(foot);
      box.classList.toggle('is-empty', !items.length);
      if (!items.length && busy) { list.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading…')); foot.hidden = true; return; }
      if (failed && !items.length) {
        list.appendChild(h('div', { class: 'empty' }, icon('alert'), h('h3', null, 'Couldn’t load the log'), h('p', null, failed),
          failedStatus === 0 || failedStatus >= 500 ? h('p', { style: 'margin-top:12px' }, h('button', { class: 'btn', type: 'button', onclick: () => load() }, icon('restart'), 'Try again')) : null));
        foot.hidden = true;
        return;
      }
      if (last && last.enabled === false) {
        list.appendChild(h('div', { class: 'banner inline msglog-off', role: 'status' }, icon('pause'),
          h('p', null, h('b', null, 'Logging is switched off. '), h('span', null, 'New deletions and edits aren’t being recorded.')),
          sectionById('msglog') ? h('a', { class: 'btn sm', href: '#/s/msglog' }, 'Settings') : null));
      }
      summary.appendChild(h('span', null, h('b', null, numberFmt.format(items.length) + ' ' + (items.length === 1 ? noun[0] : noun[1])), next ? ' so far' : '', ' in ' + periodWords() + filterWords()));
      if (!items.length) {
        const filtered = st.member || st.channel || st.q;
        list.appendChild(h('div', { class: 'empty' }, icon(tab === 'edited' ? 'edit' : 'trash'),
          h('h3', null, tab === 'edited' ? 'No edits' : 'No deleted messages'),
          h('p', null, 'Nothing in ' + periodWords() + filterWords() + '.'),
          filtered ? h('p', { class: 'hint' }, h('a', { href: hashFor({ member: '', channel: '', q: '' }) }, 'Clear the filters')) : null));
      }
      items.forEach((r) => list.appendChild(tab === 'edited' ? editedRow(r) : deletedRow(r)));
      foot.hidden = !items.length;
      if (failed) foot.appendChild(h('span', { class: 'msg-foot-error' }, icon('alert'), failed));
      if (next) foot.appendChild(h('button', { class: 'btn', type: 'button', disabled: busy, onclick: () => load() }, busy ? h('span', { class: 'spinner' }) : icon('down'), 'Load more'));
      else if (items.length) foot.appendChild(h('span', null, 'That’s everything in ' + periodWords()));
    };
    const load = async () => {
      if (busy) return;
      busy = true; failed = null;
      if (items.length) { const b = foot.querySelector('.btn'); if (b) { b.disabled = true; b.replaceChild(h('span', { class: 'spinner' }), b.firstChild); } } else draw();
      const p = new URLSearchParams({ days: st.days });
      if (st.member) p.set('member', st.member);
      if (st.channel) p.set('channel', st.channel);
      if (st.q) p.set('q', st.q);
      if (next) p.set('before', String(next));
      try {
        const data = await api('GET', '/msglog/' + tab + '?' + p.toString());
        if (!box.isConnected) return;
        if (data.member && data.member.name && !S.members.has(data.member.id)) S.members.set(data.member.id, { id: data.member.id, name: data.member.name });
        items = items.concat(data.results);
        next = data.next_before;
        last = data;
        const kept = note.querySelector('p');
        if (kept && data.log_days) kept.firstChild.textContent = 'Kept ' + plural(data.log_days, 'day') + '. ';
      } catch (e) {
        if (!box.isConnected) return;
        failed = e.message;
        failedStatus = e.status || 0;
      }
      busy = false;
      draw();
      refreshAudit();
    };
    load();
  }

  function logHead(r, extra) {
    const m = r.member;
    const ch = r.channel || {};
    const chanEl = h('span', { class: 'msg-hit-chan', title: ch.thread && ch.parent ? 'A thread in #' + ch.parent.name : ch.gone ? 'This channel no longer exists' : null },
      ch.voice ? icon('voice') : h('span', { class: 'glyph' }, '#'), ch.name,
      ch.thread && ch.parent && ch.parent.name ? h('small', null, 'thread in #' + ch.parent.name) : null,
      ch.gone ? h('small', null, 'gone') : null);
    return h('div', { class: 'msg-hit-head' },
      m ? h('a', { class: 'msg-hit-name', href: '#/members/' + m.id }, m.name) : h('span', { class: 'msg-hit-name unknown' }, 'Unknown member'),
      r.house ? h('span', { class: 'house-chip', style: '--house:' + r.house.colour }, r.house.crest + ' ' + r.house.name) : null,
      chanEl, extra || null);
  }

  function logAvatar(r) {
    const m = r.member;
    if (!m) return h('span', { class: 'msg-hit-avatar' }, h('span', { class: 'avatar lg msglog-ghost', 'aria-hidden': 'true' }, icon('user')));
    return h('a', { class: 'msg-hit-avatar', href: '#/members/' + m.id, tabindex: '-1', 'aria-hidden': 'true' }, avatar(m.avatar, m.name, 'lg'));
  }

  function logTimes(sent, verb, at) {
    const d = new Date(at * 1000);
    const whenAt = sameIstDay(sent, at) ? msgClock(at) : msgWhen(at);
    return h('p', { class: 'msglog-times' },
      h('time', { datetime: new Date(sent * 1000).toISOString(), title: fmtFull.format(new Date(sent * 1000)) + ' IST' }, 'sent ' + msgWhen(sent)),
      h('span', { class: 'sep', 'aria-hidden': 'true' }, ' · '),
      h('time', { datetime: d.toISOString(), title: fmtFull.format(d) + ' IST' }, h('b', null, verb + ' ' + whenAt)),
      h('span', { class: 'later' }, ' (' + laterWords(Math.max(0, at - sent)) + ')'));
  }

  function longText(text, cls) {
    const el = h('p', { class: cls });
    const long = Array.from(text).length > 480;
    if (!long) { el.textContent = text; return [el]; }
    const cut = Array.from(text).slice(0, 420).join('') + '…';
    el.textContent = cut;
    const more = h('button', { class: 'btn sm ghost msg-hit-more', type: 'button', 'aria-expanded': 'false', onclick: () => {
      const open = more.getAttribute('aria-expanded') !== 'true';
      el.textContent = open ? text : cut;
      more.setAttribute('aria-expanded', String(open));
      more.lastChild.textContent = open ? 'Show less' : 'Show all of it';
    } }, icon('eye'), 'Show all of it');
    return [el, more];
  }

  const LOG_REASONS = {
    before_logging: 'Sent before logging started, so there’s no copy of what it said.',
    expired: 'Sent too long before it was deleted: its copy had already been cleared.',
    missed: 'The bot didn’t catch this message when it was sent, so there’s no copy.',
  };

  function deletedRow(r) {
    const images = r.images || [];
    const files = r.files || [];
    let body;
    if (r.text === null || r.text === undefined) body = h('p', { class: 'msglog-missing' }, icon('info'), LOG_REASONS[r.reason] || 'Sent before logging started.');
    else if (!r.text.trim()) body = images.length || files.length ? null : h('p', { class: 'msglog-missing' }, '(no text)');
    else body = longText(r.text, 'msg-hit-text');
    const thumbs = images.length ? h('div', { class: 'msglog-thumbs' + (images.length === 1 ? ' one' : '') }, images.map((im, i) =>
      h('button', { class: 'msglog-thumb', type: 'button', 'aria-label': 'Open picture ' + (im.name || i + 1), onclick: () => openLightbox(images, i) },
        h('img', { src: im.url, alt: im.name || '', loading: 'lazy' })))) : null;
    const chips = files.length ? h('div', { class: 'msglog-files' }, files.map((f) =>
      h('span', { class: 'msglog-file', title: f.image ? 'This picture wasn’t saved (too big, or the download failed)' : 'Only the name is kept' },
        icon(f.image ? 'image' : 'tag'), h('span', { class: 'name' }, f.name), h('small', null, sizeWords(f.size) + (f.image ? ' · not saved' : ''))))) : null;
    return h('article', { class: 'msg-hit msglog-row' },
      logAvatar(r),
      h('div', { class: 'msg-hit-main' },
        logHead(r, r.bulk ? h('span', { class: 'badge msglog-bulk', title: 'Deleted along with other messages at once, usually by a mod or a bot' }, 'bulk delete') : null),
        logTimes(r.sent_ts, 'deleted', r.deleted_ts),
        r.reply_to ? h('p', { class: 'msg-hit-reply' }, icon('reply'), h('span', null, 'replying to ', h('b', null, '@' + (r.reply_to.author || 'someone')), r.reply_to.text ? ': “' + r.reply_to.text + '”' : '')) : null,
        body, thumbs, chips),
      h('div', { class: 'msg-hit-actions' }, r.url && !(r.channel && r.channel.gone)
        ? h('a', { class: 'btn sm', href: r.url, target: '_blank', rel: 'noopener', 'aria-label': 'Open #' + ((r.channel && r.channel.name) || 'the channel') + ' in Discord', title: 'The message is gone, so this opens the channel' }, h('span', { class: 'hide-sm' }, 'Open channel'), h('span', { class: 'show-sm' }, 'Channel'), icon('external'))
        : null));
  }

  function editedRow(r) {
    return h('article', { class: 'msg-hit msglog-row' },
      logAvatar(r),
      h('div', { class: 'msg-hit-main' },
        logHead(r),
        logTimes(r.sent_ts, 'edited', r.edited_ts),
        h('div', { class: 'msglog-diff' },
          h('div', { class: 'msglog-before' }, h('span', { class: 'msglog-label' }, 'Before'), h('del', null, longText(r.before || '', 'msglog-diff-text'))),
          h('div', { class: 'msglog-arrow', 'aria-hidden': 'true' }, icon('down')),
          h('div', { class: 'msglog-after' }, h('span', { class: 'msglog-label' }, 'After'), longText(r.after || '', 'msglog-diff-text')))),
      h('div', { class: 'msg-hit-actions' }, r.url
        ? h('a', { class: 'btn sm', href: r.url, target: '_blank', rel: 'noopener', 'aria-label': 'Open ' + ((r.member && r.member.name) || 'the') + '’s message in Discord' }, h('span', { class: 'hide-sm' }, 'Open in Discord'), h('span', { class: 'show-sm' }, 'Open'), icon('external'))
        : null));
  }

  // --- left the server -----------------------------------------------------------------

  const LEFT_PERIODS = [['7', '7 days'], ['30', '30 days'], ['90', '90 days'], ['all', 'All']];

  /** How long someone was here, in round words: "12 days", "2 months", "3 years". */
  function stayWords(secs) {
    const d = Math.round(secs / 86400);
    if (d < 1) return 'less than a day';
    if (d < 45) return plural(d, 'day');
    const m = Math.round(d / 30.44);
    if (m < 24) return plural(m, 'month');
    const y = Math.floor(d / 365.25);
    const rest = Math.round((d - y * 365.25) / 30.44);
    return plural(y, 'year') + (rest ? ' ' + plural(rest, 'month') : '');
  }

  function renderLeft(page, rq) {
    document.title = 'Left the server · Loduchand';
    const days = rq.get('days');
    const st = {
      q: (rq.get('q') || '').trim().slice(0, 100),
      days: LEFT_PERIODS.some((x) => x[0] === days) ? days : '30',
    };
    const hashFor = (over) => {
      const s = Object.assign({}, st, over || {});
      const p = new URLSearchParams();
      if (s.days !== '30') p.set('days', s.days);
      if (s.q) p.set('q', s.q);
      const qs = p.toString();
      return '#/left' + (qs ? '?' + qs : '');
    };
    const apply = (over) => navigate(hashFor(over));

    page.appendChild(pageHead('Left the server', 'Members the bot has seen leave, and what they did while they were here.'));
    page.appendChild(h('div', { class: 'banner info inline left-note', role: 'note' }, icon('info'),
      h('p', null, h('b', null, 'Only what the bot recorded. '),
        h('span', null, 'Discord tells bots nothing about people who are gone, so someone who left before the bot started watching isn’t here.'))));

    const input = h('input', { type: 'search', value: st.q, placeholder: 'Search the name the bot stored', 'aria-label': 'Filter by name', maxlength: '100', autocomplete: 'off', spellcheck: 'false', enterkeyhint: 'search' });
    input.addEventListener('search', () => { if (!input.value.trim() && st.q) apply({ q: '' }); });
    const period = segmented(LEFT_PERIODS, st.days, 'Period', (v) => apply({ days: v }));
    page.appendChild(h('form', { class: 'card msg-search left-filters', role: 'search', onsubmit: (e) => { e.preventDefault(); apply({ q: input.value.trim() }); } },
      h('div', { class: 'msg-search-row' }, h('label', { class: 'search-box' }, icon('search'), input)),
      h('div', { class: 'msg-search-filters' }, period)));

    const summary = h('div', { class: 'msg-summary', 'aria-live': 'polite' });
    const list = h('div', { class: 'msg-list' });
    const foot = h('div', { class: 'msg-foot' });
    const box = h('div', { class: 'card msg-results left-results' }, list, foot);
    page.appendChild(summary);
    page.appendChild(box);

    let items = [], next = null, total = 0, busy = false, failed = null, failedStatus = 0;
    const periodWords = () => st.days === 'all' ? 'since the bot started keeping count' : 'in the last ' + st.days + ' days';
    const nameWords = () => st.q ? ' with “' + st.q + '” in their name' : '';
    const draw = () => {
      clear(summary); clear(list); clear(foot);
      box.classList.toggle('is-empty', !items.length);
      if (!items.length && busy) { list.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading…')); foot.hidden = true; return; }
      if (failed && !items.length) {
        list.appendChild(h('div', { class: 'empty' }, icon('alert'), h('h3', null, 'Couldn’t load who left'), h('p', null, failed),
          failedStatus === 0 || failedStatus >= 500 ? h('p', { style: 'margin-top:12px' }, h('button', { class: 'btn', type: 'button', onclick: () => load() }, icon('restart'), 'Try again')) : null));
        foot.hidden = true;
        return;
      }
      summary.appendChild(h('span', null, h('b', null, plural(total, 'member') + ' left'), ' ' + periodWords() + nameWords()));
      if (!items.length) {
        list.appendChild(h('div', { class: 'empty' }, icon('userminus'), h('h3', null, 'Nobody left'),
          h('p', null, 'The bot has seen nobody go ' + periodWords() + nameWords() + '.'),
          st.q || st.days !== 'all' ? h('p', { class: 'hint' }, h('a', { href: hashFor({ q: '', days: 'all' }) }, 'Look at everyone it has seen leave')) : null));
      }
      items.forEach((r) => list.appendChild(leftRow(r)));
      foot.hidden = !items.length;
      if (failed) foot.appendChild(h('span', { class: 'msg-foot-error' }, icon('alert'), failed));
      if (next) foot.appendChild(h('button', { class: 'btn', type: 'button', disabled: busy, onclick: () => load() }, busy ? h('span', { class: 'spinner' }) : icon('down'), 'Load more'));
      else if (items.length) foot.appendChild(h('span', null, 'That’s everyone ' + periodWords()));
    };
    const load = async () => {
      if (busy) return;
      busy = true; failed = null;
      if (items.length) { const b = foot.querySelector('.btn'); if (b) { b.disabled = true; b.replaceChild(h('span', { class: 'spinner' }), b.firstChild); } } else draw();
      const p = new URLSearchParams({ days: st.days });
      if (st.q) p.set('q', st.q);
      if (next) p.set('before', String(next));
      try {
        const data = await api('GET', '/members/left?' + p.toString());
        if (!box.isConnected) return;
        items = items.concat(data.results);
        next = data.next_before;
        total = data.total;
      } catch (e) {
        if (!box.isConnected) return;
        failed = e.message;
        failedStatus = e.status || 0;
      }
      busy = false;
      draw();
      refreshAudit();
    };
    load();
  }

  function leftRow(r) {
    const href = '#/members/' + r.id;
    const facts = [h('time', { datetime: new Date(r.left_ts * 1000).toISOString(), title: fmtFull.format(new Date(r.left_ts * 1000)) + ' IST' },
      h('b', null, 'left ' + ago(r.left_ts)), ' (' + msgWhen(r.left_ts) + ')')];
    if (r.here_secs) facts.push(h('span', null, 'was here ' + stayWords(r.here_secs)));
    if (r.joins > 1 || r.leaves > 1) facts.push(h('span', null, 'joined ' + plural(r.joins, 'time') + ' · left ' + plural(r.leaves, 'time')));
    const line = h('p', { class: 'left-times' });
    facts.forEach((f, i) => { if (i) append(line, h('span', { class: 'sep', 'aria-hidden': 'true' }, ' · ')); append(line, f); });

    const chip = (ic, text, tip) => h('span', { class: 'left-stat', title: tip || null }, icon(ic), text);
    const stats = [];
    if (r.points) stats.push(chip('zap', numberFmt.format(r.points) + ' points', 'House points they earned in all their time here'));
    if (r.messages_all) stats.push(chip('message', numberFmt.format(r.messages_all) + ' messages',
      numberFmt.format(r.messages_30d) + ' of them in their last 30 days here'));
    if (r.voice_30d_min) stats.push(chip('voice', numberFmt.format(r.voice_30d_min) + ' min in voice', 'In the last 30 days they were here'));
    if (r.last_message_day) stats.push(chip('clock', 'last said something ' + dayWords(r.last_message_day), 'The day of their last counted message'));
    if (!stats.length) stats.push(h('span', { class: 'left-stat quiet' }, icon('info'), 'nothing recorded'));

    return h('article', { class: 'msg-hit left-row' },
      h('a', { class: 'msg-hit-avatar', href, tabindex: '-1', 'aria-hidden': 'true' }, avatar(r.avatar, r.name, 'lg')),
      h('div', { class: 'msg-hit-main' },
        h('div', { class: 'msg-hit-head' },
          h('a', { class: 'msg-hit-name', href }, r.name),
          r.house ? h('span', { class: 'house-chip', style: '--house:' + r.house.colour }, 'was ' + r.house.crest + ' ' + r.house.name) : null,
          r.muggle ? h('span', { class: 'badge', title: 'They had stepped out of the house game' }, 'Muggle') : null),
        line,
        h('div', { class: 'left-stats' }, stats)),
      h('div', { class: 'msg-hit-actions' }, h('a', { class: 'btn sm', href, 'aria-label': 'Open ' + r.name + '’s profile' }, 'Profile', icon('right'))));
  }

  /** "on 14 Sep" for an India day, "YYYY-MM-DD"; today and yesterday by name. */
  function dayWords(day) {
    const ts = Date.parse(day + 'T12:00:00+05:30') / 1000;
    if (!ts) return day;
    // "14/09/2026" in India time, turned round to compare with the day as stored.
    const istDay = (ms) => istParts(ms, { day: '2-digit', month: '2-digit', year: 'numeric' }).split('/').reverse().join('-');
    if (day === istDay(Date.now())) return 'today';
    if (day === istDay(Date.now() - 86400000)) return 'yesterday';
    return 'on ' + dayMonth(ts);
  }

  // --- moderation ---------------------------------------------------------------------

  const MOD_PERIODS = [['7', '7 days'], ['30', '30 days'], ['90', '90 days'], ['all', 'All']];
  const MOD_KINDS = [['', 'Everything'], ['spam', 'Spam'], ['ai', 'Possibly AI']];
  const MOD_OUTCOMES = [['', 'Any'], ['deleted', 'Deleted'], ['dismissed', 'Said not AI'], ['untouched', 'Nothing done']];

  const MOD_OUTCOME_WORDS = {
    deleted: 'deleted',
    dismissed: 'a mod said this was wrong',
    untouched: 'nobody has decided',
  };

  function renderAutomod(page, rq) {
    document.title = 'Moderation · Loduchand';
    const days = rq.get('days'), kind = rq.get('kind') || '', outcome = rq.get('outcome') || '';
    const st = {
      days: MOD_PERIODS.some((x) => x[0] === days) ? days : '30',
      kind: MOD_KINDS.some((x) => x[0] === kind) ? kind : '',
      outcome: MOD_OUTCOMES.some((x) => x[0] === outcome) ? outcome : '',
      member: (rq.get('member') || '').replace(/\D/g, '').slice(0, 20),
    };
    const hashFor = (over) => {
      const s = Object.assign({}, st, over || {});
      const p = new URLSearchParams();
      if (s.days !== '30') p.set('days', s.days);
      if (s.kind) p.set('kind', s.kind);
      if (s.outcome) p.set('outcome', s.outcome);
      if (s.member) p.set('member', s.member);
      const qs = p.toString();
      return '#/automod' + (qs ? '?' + qs : '');
    };
    const apply = (over) => navigate(hashFor(over));

    page.appendChild(pageHead('Moderation', 'What the bot removed by itself, and what it has asked a moderator to look at.',
      sectionById('automod') ? h('a', { class: 'btn', href: '#/s/automod', 'aria-label': 'Moderation settings' }, icon('sliders'), h('span', { class: 'hide-sm' }, 'Settings')) : null));

    // The rule, said plainly, above everything else on the page.
    page.appendChild(h('div', { class: 'banner info inline automod-rule', role: 'note' }, icon('shield'),
      h('p', null, h('b', null, 'Spam is deleted. A suspicion of AI writing never is. '),
        h('span', null, 'There is no reliable way to tell whether a person or a machine wrote something, and detectors are wrong most often about people writing English as a second language. An AI flag only ever asks a moderator to look; no setting changes that.'))));

    const headline = h('div', { class: 'automod-headline' });
    page.appendChild(headline);

    const switches = h('div', { class: 'automod-state' });
    page.appendChild(switches);

    const memberBtn = h('button', { class: 'picker-btn', type: 'button', 'aria-haspopup': 'listbox', 'aria-label': 'Member' });
    const drawMember = (m) => {
      clear(memberBtn);
      append(memberBtn, [st.member && m ? avatar(m.avatar, m.name, 'xs') : icon(st.member ? 'user' : 'users'),
        h('span', { class: 'value' + (st.member ? '' : ' placeholder') }, st.member ? (m ? m.name : 'Member ' + st.member) : 'Any member'), icon('chevron')]);
    };
    drawMember(null);
    if (st.member) memberById(st.member).then((m) => { if (memberBtn.isConnected) drawMember(m); });
    memberBtn.addEventListener('click', () => openPicker(memberBtn, { title: 'Member', placeholder: 'Search members by name', debounce: 180,
      load: async (q) => (q ? [] : [{ id: '', label: 'Any member', lead: icon('users'), trail: !st.member ? icon('check', 'check-mark') : null }]).concat(await memberItems(q)),
      onPick: (it) => apply({ member: it.id }) }));

    page.appendChild(h('div', { class: 'card msg-search automod-filters' },
      h('div', { class: 'msg-search-filters' },
        segmented(MOD_KINDS, st.kind, 'Kind', (v) => apply({ kind: v })),
        memberBtn,
        segmented(MOD_OUTCOMES, st.outcome, 'What happened', (v) => apply({ outcome: v })),
        segmented(MOD_PERIODS, st.days, 'Period', (v) => apply({ days: v })))));

    const summary = h('div', { class: 'msg-summary', 'aria-live': 'polite' });
    const list = h('div', { class: 'msg-list' });
    const foot = h('div', { class: 'msg-foot' });
    const box = h('div', { class: 'card msg-results automod-results' }, list, foot);
    page.appendChild(summary);
    page.appendChild(box);
    const people = h('div', { class: 'automod-people' });
    page.appendChild(people);

    let items = [], next = null, last = null, busy = false, failed = null, failedStatus = 0;
    const periodWords = () => st.days === 'all' ? 'since the bot started watching' : 'in the last ' + st.days + ' days';
    const filterWords = () => {
      const bits = [];
      if (st.kind) bits.push(st.kind === 'ai' ? 'possibly AI' : 'spam');
      if (st.member) { const m = S.members.get(st.member); bits.push('from ' + (m ? m.name : 'that member')); }
      if (st.outcome) bits.push(MOD_OUTCOME_WORDS[st.outcome]);
      return bits.length ? ' (' + bits.join(', ') + ')' : '';
    };

    const drawHeadline = () => {
      clear(headline);
      if (!last) return;
      const n = last.not_ai || {};
      const judged = n.decided || 0;
      const card = h('div', { class: 'automod-wrong' + (judged && n.pct >= 25 ? ' bad' : '') });
      if (!judged) {
        append(card, [h('span', { class: 'automod-wrong-num' }, '—'),
          h('div', null, h('b', null, 'No AI flag has been judged yet.'),
            h('span', null, 'Until a moderator presses Not AI or Delete it on one, this feature has no measured accuracy at all — and a zero here would be flattering it.'))]);
      } else {
        append(card, [h('span', { class: 'automod-wrong-num' }, Math.round(n.pct) + '%'),
          h('div', null, h('b', null, 'Moderators said “not AI” on ' + n.dismissed + ' of the ' + plural(judged, 'flag') + ' they judged.'),
            h('span', null, 'This is the only honest measure of whether the AI half is worth keeping. If it stays high, switch it off.'))]);
      }
      headline.appendChild(card);
      const t = last.totals || {};
      headline.appendChild(h('div', { class: 'automod-tiles' },
        h('div', { class: 'automod-tile' }, h('b', null, numberFmt.format(t.spam || 0)), h('span', null, 'spam removals')),
        h('div', { class: 'automod-tile' }, h('b', null, numberFmt.format(t.ai || 0)), h('span', null, 'AI flags')),
        h('div', { class: 'automod-tile' }, h('b', null, numberFmt.format(t.untouched || 0)), h('span', null, 'still waiting on a mod'))));
    };

    const drawState = () => {
      clear(switches);
      if (!last) return;
      const chip = (on, label, why) => h('span', { class: 'automod-chip' + (on ? ' on' : ''), title: why }, icon(on ? 'check' : 'pause'), label + (on ? ' on' : ' off'));
      if (!last.enabled) {
        switches.appendChild(h('div', { class: 'banner inline', role: 'status' }, icon('pause'),
          h('p', null, h('b', null, 'Moderation is switched off. '), h('span', null, 'Nothing is being watched, removed or flagged.')),
          sectionById('automod') ? h('a', { class: 'btn sm', href: '#/s/automod' }, 'Settings') : null));
        return;
      }
      append(switches, [chip(last.spam_on, 'Deleting spam', 'The objective rules that remove messages'),
        chip(last.ai_on, 'Flagging AI', 'Puts long messages in front of a moderator; never deletes'),
        h('span', { class: 'automod-chip quiet', title: 'How high the free score has to be before a moderator is asked' }, icon('target'), 'flags at ' + (last.threshold || 0)),
        h('span', { class: 'automod-chip quiet', title: 'After this the words are cleared; the record of what happened stays' }, icon('clock'), 'text kept ' + plural(last.keep_days || 30, 'day'))]);
    };

    const drawPeople = () => {
      clear(people);
      if (!last || !(last.by_member || []).length) return;
      const rows = last.by_member.map((m) => h('li', null,
        h('a', { class: 'inline-ref', href: '#/members/' + m.member.id }, avatar(m.member.avatar, m.member.name, 'xs'), m.member.name),
        h('span', { class: 'grow' }),
        m.spam ? h('span', { class: 'badge' }, plural(m.spam, 'spam removal') ) : null,
        m.ai ? h('span', { class: 'badge' }, plural(m.ai, 'AI flag')) : null,
        m.dismissed ? h('span', { class: 'badge good', title: 'A moderator said the flag was wrong' }, m.dismissed + ' wrong') : null));
      people.appendChild(card('automod-people', 'Who has been flagged', 'Counted ' + periodWords() + '. A row here is not a verdict about anybody.', h('ul', { class: 'pins' }, rows)));
    };

    const draw = () => {
      clear(summary); clear(list); clear(foot);
      box.classList.toggle('is-empty', !items.length);
      drawHeadline();
      drawState();
      drawPeople();
      if (!items.length && busy) { list.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading…')); foot.hidden = true; return; }
      if (failed && !items.length) {
        list.appendChild(h('div', { class: 'empty' }, icon('alert'), h('h3', null, 'Couldn’t load the moderation log'), h('p', null, failed),
          failedStatus === 0 || failedStatus >= 500 ? h('p', { style: 'margin-top:12px' }, h('button', { class: 'btn', type: 'button', onclick: () => load() }, icon('restart'), 'Try again')) : null));
        foot.hidden = true;
        return;
      }
      summary.appendChild(h('span', null, h('b', null, numberFmt.format(items.length) + ' ' + plural(items.length, 'flag').replace(/^\d+\s/, '')), next ? ' so far' : '', ' ' + periodWords() + filterWords()));
      if (!items.length) {
        const filtered = st.kind || st.member || st.outcome;
        list.appendChild(h('div', { class: 'empty' }, icon('shield'), h('h3', null, 'Nothing flagged'),
          h('p', null, 'Nothing was removed or questioned ' + periodWords() + filterWords() + '.'),
          filtered ? h('p', { class: 'hint' }, h('a', { href: hashFor({ kind: '', member: '', outcome: '' }) }, 'Clear the filters')) : null));
      }
      items.forEach((r) => list.appendChild(flagRow(r)));
      foot.hidden = !items.length;
      if (failed) foot.appendChild(h('span', { class: 'msg-foot-error' }, icon('alert'), failed));
      if (next) foot.appendChild(h('button', { class: 'btn', type: 'button', disabled: busy, onclick: () => load() }, busy ? h('span', { class: 'spinner' }) : icon('down'), 'Load more'));
      else if (items.length) foot.appendChild(h('span', null, 'That’s everything ' + periodWords()));
    };

    const load = async () => {
      if (busy) return;
      busy = true; failed = null;
      if (items.length) { const b = foot.querySelector('.btn'); if (b) { b.disabled = true; b.replaceChild(h('span', { class: 'spinner' }), b.firstChild); } } else draw();
      const p = new URLSearchParams({ days: st.days });
      if (st.kind) p.set('kind', st.kind);
      if (st.member) p.set('member', st.member);
      if (st.outcome) p.set('outcome', st.outcome);
      if (next) p.set('before', String(next));
      try {
        const data = await api('GET', '/automod?' + p.toString());
        if (!box.isConnected) return;
        if (data.member && data.member.name && !S.members.has(data.member.id)) S.members.set(data.member.id, { id: data.member.id, name: data.member.name });
        items = items.concat(data.results);
        next = data.next_before;
        last = data;
      } catch (e) {
        if (!box.isConnected) return;
        failed = e.message;
        failedStatus = e.status || 0;
      }
      busy = false;
      draw();
      refreshAudit();
    };
    load();
  }

  function flagRow(r) {
    const ai = r.kind === 'ai';
    const m = r.member || {};
    const href = '#/members/' + m.id;
    const badge = ai
      ? h('span', { class: 'badge automod-kind ai', title: 'Only flagged for a human. Nothing was deleted for this.' }, icon('bot'), 'possibly AI')
      : h('span', { class: 'badge automod-kind spam', title: 'An objective rule. These messages were removed.' }, icon('trash'), 'spam');
    const outcome = h('span', { class: 'badge automod-outcome ' + r.outcome, title: ai && r.outcome === 'dismissed' ? 'Counted against this feature' : null },
      MOD_OUTCOME_WORDS[r.outcome] || r.outcome,
      r.decided_by ? h('small', null, ' · ' + r.decided_by.name) : null);

    const facts = [h('span', null, r.rule_label)];
    if (r.messages > 1) facts.push(h('span', null, plural(r.messages, 'message') + ' removed'));
    if (ai && r.score !== null && r.score !== undefined) facts.push(h('span', null, 'score ' + r.score.toFixed(2)));
    if (ai && !r.had_baseline) facts.push(h('span', { class: 'quiet', title: 'The bot had not seen enough of their writing to compare this with their usual style' }, 'no style baseline'));
    const line = h('p', { class: 'msglog-times' });
    const when = new Date(r.ts * 1000);
    append(line, h('time', { datetime: when.toISOString(), title: fmtFull.format(when) + ' IST' }, msgWhen(r.ts)));
    facts.forEach((f) => { append(line, h('span', { class: 'sep', 'aria-hidden': 'true' }, ' · ')); append(line, f); });

    const reasons = (r.reasons || []).length
      ? h('ul', { class: 'automod-reasons' }, r.reasons.map((why) => h('li', null, why)))
      : null;
    const model = r.model
      ? h('p', { class: 'automod-model' }, icon('bot'),
          h('span', null, r.model.unjudgeable
            ? r.model.name + ' said this text can’t be judged either way'
            : r.model.name + ' was ' + r.model.confidence.toFixed(2) + ' sure' + (r.model.reasons ? ' — ' + r.model.reasons : '')))
      : (ai ? h('p', { class: 'automod-model quiet' }, icon('bot'), h('span', null, 'no second opinion was asked for')) : null);

    const body = r.text_cleared
      ? h('p', { class: 'msglog-missing' }, icon('info'), 'The words were cleared when this got old. What happened is still on the record.')
      : (r.text ? longText(r.text, 'msg-hit-text') : h('p', { class: 'msglog-missing' }, '(no text)'));

    return h('article', { class: 'msg-hit automod-row' + (ai ? ' is-ai' : '') },
      h('a', { class: 'msg-hit-avatar', href, tabindex: '-1', 'aria-hidden': 'true' }, avatar(m.avatar, m.name, 'lg')),
      h('div', { class: 'msg-hit-main' },
        h('div', { class: 'msg-hit-head' },
          h('a', { class: 'msg-hit-name', href }, m.name),
          badge,
          h('span', { class: 'msg-hit-chan' }, h('span', { class: 'glyph' }, '#'), (r.channel || {}).name),
          outcome),
        line, body, reasons, model),
      h('div', { class: 'msg-hit-actions' }, r.url
        ? h('a', { class: 'btn sm', href: r.url, target: '_blank', rel: 'noopener', 'aria-label': 'Open the conversation in Discord', title: 'Opens where it happened; a deleted message is gone' }, h('span', { class: 'hide-sm' }, 'In Discord'), h('span', { class: 'show-sm' }, 'Open'), icon('external'))
        : null));
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
    return String(line).split(/(\{(?:name|mention|hours|days|date|day|time|leader|standings|countdown:[^{}]*|random:[^{}]*)\})/g).map((part, i) => (i % 2 ? h('span', { class: 'ph-inline' }, part) : part));
  }

  // --- richer posts: pictures, placeholders, the Discord preview ------------------------

  async function loadMedia(force) {
    if (S.media && !force) return S.media;
    try { S.media = (await api('GET', '/media')).items; } catch (_) { S.media = S.media || []; }
    return S.media;
  }
  async function loadPlaceholders() {
    if (S.placeholders && Date.now() - S.placeholders.at < 60000) return S.placeholders;
    try { S.placeholders = Object.assign(await api('GET', '/reminders/placeholders'), { at: Date.now() }); } catch (_) { /* the preview shows samples */ }
    return S.placeholders;
  }
  const mediaById = (id) => (S.media || []).find((m) => m.id === id);
  function mediaImg(id, cls, alt) {
    return h('img', { class: cls || '', src: '/api/media/' + encodeURIComponent(id), alt: alt || '', loading: 'lazy', decoding: 'async' });
  }
  function fileSize(n) { return n >= 1048576 ? (Math.round(n / 104857.6) / 10) + ' MB' : Math.max(1, Math.round(n / 1024)) + ' KB'; }

  /** India's today as YYYY-MM-DD. */
  function istToday() {
    const p = new Intl.DateTimeFormat('en-CA', { timeZone: IST, year: 'numeric', month: '2-digit', day: '2-digit' }).format(new Date());
    return (S.placeholders && S.placeholders.today) || p;
  }
  function addDays(ymd, n) { const [y, m, d] = ymd.split('-').map(Number); return new Date(Date.UTC(y, m - 1, d + n)).toISOString().slice(0, 10); }
  function countdownWords(ymd, today) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(String(ymd).trim())) return null;
    const [y, m, d] = String(ymd).trim().split('-').map(Number);
    const [ty, tm, td] = (today || istToday()).split('-').map(Number);
    const days = Math.round((Date.UTC(y, m - 1, d) - Date.UTC(ty, tm - 1, td)) / 86400000);
    if (isNaN(days)) return null;
    return days <= 0 ? 'today' : days === 1 ? '1 day' : days + ' days';
  }
  function fromNow(ts) {
    const s = Math.round(ts - Date.now() / 1000);
    if (s < 60) return 'in under a minute';
    if (s < 3600) return 'in ' + Math.round(s / 60) + ' min';
    if (s < 86400) { const total = Math.round(s / 60), hrs = Math.floor(total / 60), min = total % 60; return 'in ' + hrs + ' h' + (min ? ' ' + min + ' min' : ''); }
    const days = Math.round(s / 86400);
    return days === 1 ? 'in a day' : 'in ' + days + ' days';
  }

  const istParts = (ms, opts) => new Intl.DateTimeFormat('en-GB', Object.assign({ timeZone: IST }, opts)).format(new Date(ms));
  /**
   * `{date}`, `{standings}`, `{countdown:…}`, `{random:…}` filled the way the bot fills them.
   * `pick` steps the random choices; `at` (ms) is when the post goes out, for the date and time.
   */
  function fillExtras(text, pick, at) {
    const P = Object.assign({}, S.placeholders || {});
    if (at) {
      // Built by hand: browsers disagree on "Sep" and "Sept"; the bot writes "Mon 14 Sep".
      const [wd, dd, mm] = [istParts(at, { weekday: 'short' }), istParts(at, { day: 'numeric' }), +new Intl.DateTimeFormat('en-GB', { timeZone: IST, month: 'numeric' }).format(new Date(at))];
      P.date = wd.slice(0, 3) + ' ' + dd + ' ' + ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][mm - 1];
      P.day = istParts(at, { weekday: 'long' });
      P.time = istParts(at, { hour: '2-digit', minute: '2-digit', hour12: false });
      P.today = new Intl.DateTimeFormat('en-CA', { timeZone: IST, year: 'numeric', month: '2-digit', day: '2-digit' }).format(new Date(at));
    }
    let out = '', rest = String(text || ''), n = 0;
    for (;;) {
      const start = rest.indexOf('{');
      if (start < 0) break;
      let depth = 0, end = -1;
      for (let i = start + 1; i < rest.length; i++) {
        if (rest[i] === '{') depth++;
        else if (rest[i] === '}') { if (!depth) { end = i; break; } depth--; }
      }
      if (end < 0) break;
      out += rest.slice(0, start);
      const inner = rest.slice(start + 1, end);
      if (inner.startsWith('random:')) { const opts = inner.slice(7).split('|'); out += opts[((pick || 0) + n++) % opts.length].trim(); }
      else if (inner.startsWith('countdown:')) { const w = countdownWords(inner.slice(10), P.today); out += w === null ? rest.slice(start, end + 1) : w; }
      else out += rest.slice(start, end + 1);
      rest = rest.slice(end + 1);
    }
    out += rest;
    const val = (k, sample) => (P[k] !== undefined ? P[k] : sample);
    return out.replace(/\{date\}/g, val('date', 'Mon 14 Sep')).replace(/\{day\}/g, val('day', 'Monday')).replace(/\{time\}/g, val('time', '21:00'))
      .replace(/\{leader\}/g, val('leader', '🦅 Ravenclaw')).replace(/\{standings\}/g, val('standings', '🦅 413 · 🦁 397 · 🐍 249 · 🦡 249'));
  }

  /** Discord's **bold** in text pieces; mentions and other nodes pass through. */
  function discordMarkup(parts) {
    const out = [];
    [].concat(parts).forEach((p) => {
      if (typeof p !== 'string') { out.push(p); return; }
      p.split(/(\*\*[^*\n]+\*\*)/g).forEach((bit, i) => { if (!bit) return; out.push(i % 2 ? h('strong', null, bit.slice(2, -2)) : bit); });
    });
    return out;
  }

  /** One bot message as Discord shows it: text or a card, the picture, the reactions. */
  function postMessage(p) {
    const body = [];
    if (p.style === 'card') {
      const colour = /^#[0-9a-f]{6}$/i.test(p.colour || '') ? p.colour : '#8b93ff';
      body.push(h('div', { class: 'embed', style: '--embed:' + colour },
        p.title ? h('div', { class: 'embed-title' }, discordMarkup(p.title)) : null,
        p.text && [].concat(p.text).length ? h('div', { class: 'embed-desc' }, discordMarkup(p.text)) : null,
        p.image ? h('div', { class: 'embed-image' }, p.image) : null,
        p.footer ? h('div', { class: 'embed-footer' }, p.footer) : null));
    } else {
      if (p.text && [].concat(p.text).some((x) => x && (typeof x !== 'string' || x.trim()))) body.push(h('div', { class: 'msg-text' }, discordMarkup(p.text)));
      if (p.image) body.push(h('div', { class: 'msg-attach' }, p.image));
    }
    return h('div', { class: 'msg' }, h('span', { class: 'brand-mark' }, h('span', null, 'L')),
      h('div', { style: 'min-width:0' }, h('div', { class: 'msg-head' }, h('b', null, 'Loduchand'), h('span', { class: 'msg-bot' }, 'BOT'), h('span', { class: 'msg-time' }, p.time || 'Today')),
        body,
        p.reactions && p.reactions.length ? h('div', { class: 'msg-reactions' }, p.reactions.map((r) => h('span', { class: 'msg-reaction' }, emojiEl(r), h('b', null, '1')))) : null));
  }

  /** Uploads picture files one by one; returns the saved ones. */
  async function uploadPictures(files) {
    const saved = [];
    for (const file of Array.from(files || [])) {
      if (!/^image\/(png|jpeg|gif|webp)$/.test(file.type)) { toast(file.name + ': only PNG, JPG, GIF and WebP pictures can be used', 'error'); continue; }
      if (file.size > 8 * 1048576) { toast(file.name + ' is ' + fileSize(file.size) + '. Pictures can be at most 8 MB.', 'error'); continue; }
      let res, data = null;
      try {
        res = await fetch('/api/media', { method: 'POST', credentials: 'same-origin', headers: { 'X-Panel': '1', 'Content-Type': file.type, 'X-Filename': encodeURIComponent(file.name) }, body: file });
        try { data = await res.json(); } catch (_) { /* empty */ }
      } catch (_) { toast("Can't reach the bot. It may be restarting.", 'error'); break; }
      if (res.status === 401) { renderSignIn({ error: 'Your session has ended. Run /panel in Discord to sign in again.' }); break; }
      if (!res.ok) { toast(file.name + ': ' + ((data && data.error) || 'upload failed (' + res.status + ')'), 'error'); continue; }
      saved.push(data);
      S.media = [data].concat((S.media || []).filter((m) => m.id !== data.id));
    }
    if (saved.length) refreshAudit();
    return saved;
  }

  /** A drop target that also opens the file chooser. */
  function dropZone(label, sub, onFiles) {
    const input = h('input', { type: 'file', accept: 'image/png,image/jpeg,image/gif,image/webp', multiple: true, hidden: true });
    const zone = h('button', { class: 'dropzone', type: 'button' }, icon('upload'), h('span', null, h('b', null, label), h('small', null, sub)), input);
    zone.addEventListener('click', (e) => { if (e.target !== input) input.click(); });
    input.addEventListener('change', () => { const files = Array.from(input.files); input.value = ''; if (files.length) onFiles(files); });
    ['dragenter', 'dragover'].forEach((t) => zone.addEventListener(t, (e) => { e.preventDefault(); zone.classList.add('over'); }));
    ['dragleave', 'drop'].forEach((t) => zone.addEventListener(t, (e) => { e.preventDefault(); zone.classList.remove('over'); }));
    zone.addEventListener('drop', (e) => { const files = Array.from((e.dataTransfer && e.dataTransfer.files) || []); if (files.length) onFiles(files); });
    return zone;
  }

  /** The picture library: pick for a reminder (`selected` is its list), upload, delete. */
  async function openLibrary(selected, onDone) {
    const sheet = openSheet('Picture library', 'PNG, JPG, GIF or WebP, up to 8 MB each. Only admins can see these until they are posted.', onDone);
    const grid = h('div', { class: 'lib-grid' });
    const count = h('span', { class: 'count' });
    let busy = false;
    const draw = () => {
      clear(grid);
      const all = S.media || [];
      count.textContent = plural(all.length, 'picture') + (selected ? ' · ' + selected.length + ' picked' : '');
      if (!all.length) grid.appendChild(h('p', { class: 'hint', style: 'grid-column:1/-1;margin:0' }, 'No pictures yet. Upload a few above.'));
      all.forEach((m) => {
        const on = selected && selected.includes(m.id);
        const used = (m.used_by || []).map((u) => u.name);
        const tile = h('div', { class: 'lib-tile' + (on ? ' is-on' : '') },
          h('button', { class: 'lib-pick', type: 'button', 'aria-pressed': selected ? String(!!on) : null, 'aria-label': (on ? 'Remove ' : 'Use ') + m.name,
            onclick: () => { if (!selected) return; const i = selected.indexOf(m.id); if (i >= 0) selected.splice(i, 1); else selected.push(m.id); draw(); } },
            mediaImg(m.id, 'lib-img', m.name), on ? h('span', { class: 'lib-check' }, icon('check'), String(selected.indexOf(m.id) + 1)) : null),
          h('div', { class: 'lib-meta' }, h('span', { class: 'lib-name', title: m.name }, m.name),
            h('small', null, fileSize(m.size) + (m.mime === 'image/gif' ? ' · GIF' : '') + (used.length ? ' · in ' + used.join(', ') : '')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Delete ' + m.name, disabled: used.length > 0, 'data-tip': used.length ? 'Used by ' + used.join(', ') : null, onclick: async () => {
              const ok = await confirmDialog({ title: 'Delete “' + m.name + '”?', icon: 'trash', danger: true, body: 'It goes from the library for good. Posts already in Discord keep their copy.', confirm: 'Delete picture' });
              if (!ok) return;
              try { await api('DELETE', '/media/' + m.id); S.media = S.media.filter((x) => x.id !== m.id); if (selected) { const i = selected.indexOf(m.id); if (i >= 0) selected.splice(i, 1); } draw(); toast('Deleted ' + m.name); refreshAudit(); }
              catch (e) { toast(e.message, 'error'); }
            } }, icon('trash'))));
        grid.appendChild(tile);
      });
    };
    const upload = async (files) => {
      if (busy) return;
      busy = true; zone.classList.add('busy');
      const saved = await uploadPictures(files);
      if (selected) saved.forEach((m) => { if (!selected.includes(m.id)) selected.push(m.id); });
      busy = false; zone.classList.remove('busy');
      draw();
    };
    const zone = dropZone('Upload pictures', 'Drop them here or choose files', upload);
    append(sheet.body, [zone, h('div', { class: 'toolbar', style: 'margin:0' }, count, h('span', { class: 'grow' }), selected ? h('button', { class: 'btn primary sm', type: 'button', onclick: () => sheet.close() }, 'Done') : null), grid]);
    draw();
    await loadMedia(true);
    if (grid.isConnected) draw();
  }

  const PLACEHOLDERS = [
    ['{name}', 'The display name below'], ['{mention}', 'Pings the member (plain text only)'], ['{hours}', 'Hours since “since”'], ['{days}', 'Days since “since”'],
    ['{date}', 'Like Mon 14 Sep'], ['{day}', 'Like Monday'], ['{time}', 'The time it posts, IST'], ['{leader}', 'This month’s House Cup leader'],
    ['{standings}', '🦅 413 · 🦁 397 · 🐍 249 · 🦡 249'], ['{countdown}', 'Days until a date: change the date after inserting'], ['{random}', 'One of the choices, picked each time'],
  ];

  // --- reminders page ------------------------------------------------------------------

  function reminderTabs(active) {
    const pending = S.memoPending;
    return h('nav', { class: 'page-tabs', 'aria-label': 'Reminder views' },
      h('a', { href: '#/reminders', 'aria-current': active === 'posts' ? 'page' : null }, icon('calendar'), h('span', { class: 'hide-sm' }, 'Scheduled posts'), h('span', { class: 'show-sm' }, 'Posts'), h('span', { class: 'tab-count' }, String(S.reminders.length))),
      h('a', { href: '#/reminders/members', 'aria-current': active === 'members' ? 'page' : null }, icon('users'), h('span', { class: 'hide-sm' }, 'Members’ reminders'), h('span', { class: 'show-sm' }, 'Members'),
        h('span', { class: 'tab-count' + (pending ? ' is-live' : ''), id: 'memo-tab-count', hidden: pending === undefined }, String(pending || 0))));
  }
  async function refreshMemoCount() {
    try {
      const data = await api('GET', '/memos?status=pending');
      S.memoPending = data.pending;
      const el = document.getElementById('memo-tab-count');
      if (el) { el.textContent = String(data.pending); el.hidden = false; el.classList.toggle('is-live', data.pending > 0); }
      return data;
    } catch (_) { return null; }
  }

  function renderReminders(page) {
    document.title = 'Reminders · Loduchand';
    const newBtn = h('button', { class: 'btn primary', type: 'button', onclick: () => chooseTemplate() }, icon('plus'), 'New post');
    page.appendChild(pageHead('Reminders', 'Posts Loduchand makes on a schedule, and the reminders members set for themselves.', newBtn));
    page.appendChild(reminderTabs('posts'));
    const grid = h('div', { class: 'reminders' });
    const draw = () => {
      clear(grid);
      S.reminders.forEach((r) => grid.appendChild(reminderCard(r)));
      grid.appendChild(h('button', { class: 'new-card', type: 'button', onclick: () => chooseTemplate() }, icon('plus'), S.reminders.length ? 'New post' : 'Make your first scheduled post', h('small', null, 'Start blank or from a template')));
    };
    draw();
    page.appendChild(grid);
    refreshMemoCount();
    const needs = S.reminders.some((r) => (r.images || []).length || (r.reactions || []).some((x) => /:\d+>?$/.test(x)));
    if (needs && (!S.media || !S.emojis)) Promise.all([loadMedia(), loadEmojis()]).then(() => { if (grid.isConnected) draw(); });
  }

  async function chooseTemplate() {
    const sheet = openSheet('Start from…', 'Pick a template to fill in the editor, or start blank. Nothing is saved until you save.');
    const list = h('div', { class: 'tpl-list' });
    const go = (key) => { sheet.close(); navigate('#/reminders/new' + (key ? '?t=' + key : '')); };
    list.appendChild(h('button', { class: 'tpl', type: 'button', onclick: () => go('') }, h('span', { class: 'tpl-icon', 'aria-hidden': 'true' }, icon('plus')), h('span', { class: 'grow' }, h('b', null, 'Blank'), h('small', null, 'Your own lines, schedule and look.'))));
    sheet.body.appendChild(list);
    if (!S.templates) {
      const wait = h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading templates…');
      sheet.body.appendChild(wait);
      try { S.templates = await api('GET', '/reminders/templates'); } catch (e) { toast(e.message, 'error'); S.templates = []; }
      wait.remove();
    }
    S.templates.forEach((t) => {
      const r = t.reminder;
      const bits = [r.style === 'card' ? 'Card' : 'Plain text', r.ai_prompt ? 'AI-written' : null, r.image_order === 'random' || t.key === 'good_morning' ? 'Pictures' : null, scheduleWords(r.schedule)].filter(Boolean);
      list.appendChild(h('button', { class: 'tpl', type: 'button', onclick: () => go(t.key) }, h('span', { class: 'tpl-icon', 'aria-hidden': 'true' }, t.icon),
        h('span', { class: 'grow' }, h('b', null, t.name), h('small', null, t.about), h('span', { class: 'tpl-tags' }, bits.map((b) => h('span', { class: 'badge' }, b))))));
    });
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
    const images = r.images || [];
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
    if ((r.reactions || []).length || r.delete_previous) facts.appendChild(h('li', null, icon('smile'), h('span', { class: 'reaction-row' }, (r.reactions || []).map(emojiEl), r.delete_previous ? h('span', { style: 'color:var(--faint)' }, ((r.reactions || []).length ? ' · ' : '') + 'keeps only the latest post') : null)));
    const badges = [
      r.enabled ? h('span', { class: 'badge on' }, h('span', { class: 'dot' }), 'Running') : h('span', { class: 'badge paused' }, 'Paused'),
      r.style === 'card' ? h('span', { class: 'badge kind' }, h('span', { class: 'swatch', style: 'background:' + (/^#[0-9a-f]{6}$/i.test(r.colour || '') ? r.colour : '#8b93ff') }), 'Card') : null,
      r.ai_prompt ? h('span', { class: 'badge kind' }, icon('spark'), 'AI') : null,
      images.length ? h('span', { class: 'badge kind', title: plural(images.length, 'picture') }, '🖼️ ' + images.length) : null,
      r.lines.length ? h('span', { class: 'badge' }, icon(r.order === 'random' ? 'shuffle' : 'repeat'), plural(r.lines.length, 'line')) : null,
    ];
    el.appendChild(h('div', { class: 'reminder-head' },
      images.length ? h('a', { class: 'reminder-thumb', href: '#/reminders/' + r.id, 'aria-label': 'Edit ' + r.name, tabindex: '-1' }, mediaImg(images[0]), images.length > 1 ? h('span', null, '+' + (images.length - 1)) : null) : null,
      h('div', { class: 'grow' }, h('h3', null, r.name), h('div', { class: 'sub' }, badges)),
      sw));
    el.appendChild(facts);
    if (r.ai_prompt) el.appendChild(h('p', { class: 'reminder-quote ai', title: r.ai_prompt }, icon('spark'), ' ', r.ai_prompt));
    else if (r.style === 'card' && r.title) el.appendChild(h('p', { class: 'reminder-quote', title: r.title, style: 'border-left-color:' + (/^#[0-9a-f]{6}$/i.test(r.colour || '') ? r.colour : '#8b93ff') }, h('b', null, templated(r.title)), r.lines[0] ? [' · ', templated(r.lines[0].split('\n')[0].replace(/\*\*/g, ''))] : null));
    else if (r.lines[0]) el.appendChild(h('p', { class: 'reminder-quote', title: r.lines[0] }, templated(r.lines[0].replace(/\*\*/g, ''))));
    el.appendChild(h('div', { class: 'reminder-foot' },
      h('span', { class: 'grow' }, r.sent_count ? 'Sent ' + plural(r.sent_count, 'time') + (r.last_sent ? ' · last ' + ago(r.last_sent) : '') : 'Not sent yet'),
      h('a', { class: 'btn sm', href: '#/reminders/' + r.id }, icon('edit'), 'Edit')));
    return el;
  }

  function blankReminder() {
    const general = S.channels.find((c) => c.kind === 'text' && c.name === 'general');
    return { id: 0, name: '', enabled: true, channel_id: general ? general.id : '', lines: [''], order: 'rotate', schedule: { kind: 'every', minutes: 60 },
      active_from: '', active_to: '', user_id: '', user_name: '', since: '', stop_when_back: false, welcome_line: '', ends: '', last_sent: 0, sent_count: 0,
      images: [], image_order: 'rotate', style: 'plain', title: '', colour: '', footer: '', reactions: [], delete_previous: false, last_message_id: '', ai_prompt: '', ai_recent: [] };
  }

  // --- members' reminders --------------------------------------------------------------

  const memoView = { status: 'pending', q: '' };
  const VIA = { chat: ['Asked the bot', 'message'], command: ['/remind', 'slash'], panel: ['Panel', 'sliders'] };
  const MEMO_STATUS = { pending: ['Waiting', 'waiting'], sent: ['Sent', 'on'], cancelled: ['Cancelled', 'paused'], failed: ['Not delivered', 'failed'] };

  function memberRemindersSetting() {
    for (const sec of S.sections) { const st = sec.settings.find((x) => x.key === 'VIZIER_MEMBER_REMINDERS'); if (st) return { sec, st }; }
    return null;
  }

  function renderMemos(page) {
    document.title = 'Members’ reminders · Loduchand';
    page.appendChild(pageHead('Reminders', 'Posts Loduchand makes on a schedule, and the reminders members set for themselves.',
      h('button', { class: 'btn primary', type: 'button', onclick: () => openMemoForm(() => load()) }, icon('plus'), 'New member reminder')));
    page.appendChild(reminderTabs('members'));
    const found = memberRemindersSetting();
    const settingLink = found ? h('a', { href: '#/s/' + found.sec.id + '?k=VIZIER_MEMBER_REMINDERS' }, h('code', null, 'VIZIER_MEMBER_REMINDERS')) : h('code', null, 'VIZIER_MEMBER_REMINDERS');
    const banner = h('div');
    page.appendChild(banner);
    const input = h('input', { type: 'search', placeholder: 'Search by member, text or channel', 'aria-label': 'Search reminders', value: memoView.q });
    const count = h('span', { class: 'count', 'aria-live': 'polite' });
    const list = h('div', { class: 'memo-list card' }, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading reminders…'));
    let data = null;
    const drawBanner = () => {
      clear(banner);
      const on = data ? data.enabled : true;
      banner.appendChild(h('div', { class: 'memo-note' + (on ? '' : ' is-off') }, icon(on ? 'info' : 'pause'),
        h('p', null, on ? null : h('b', null, 'Member reminders are switched off, so none of these go out. '),
          'Members set these by asking the bot (“remind me in 2 hours to…”) or with ', h('code', null, '/remind'), '. The master switch is ', settingLink, '.')));
    };
    const draw = () => {
      clear(list);
      if (!data) return;
      const q = memoView.q.trim().toLowerCase();
      const items = data.items.filter((m) => !q || [m.user && m.user.name, m.set_by && m.set_by.name, m.text, m.channel && m.channel.name, (VIA[m.via] || [m.via])[0]].filter(Boolean).join(' ').toLowerCase().includes(q));
      count.textContent = items.length === data.items.length ? plural(items.length, 'reminder') : items.length + ' of ' + data.items.length;
      if (!items.length) {
        list.appendChild(h('div', { class: 'empty' }, h('p', null, q ? 'No reminder matches “' + memoView.q.trim() + '”.' : memoView.status === 'pending' ? 'Nobody has a reminder waiting.' : 'Nothing here yet.'),
          !q && memoView.status === 'pending' ? h('small', null, 'Members can ask: “@Loduchand remind me at 9pm to join quiz”.') : null));
        return;
      }
      items.forEach((m) => list.appendChild(memoRow(m, async () => {
        const who = (m.user && m.user.name) || 'this member';
        const ok = await confirmDialog({ title: 'Cancel ' + who + '’s reminder?', icon: 'trash', danger: true, body: 'It was due ' + m.due_words + '. It won’t be sent, and ' + who + ' isn’t told.', confirm: 'Cancel reminder', cancel: 'Keep it' });
        if (!ok) return;
        try { await api('POST', '/memos/' + m.id + '/cancel'); toast('Cancelled ' + who + '’s reminder'); refreshAudit(); load(); }
        catch (e) { toast(e.message, 'error'); load(); }
      })));
    };
    const load = async () => {
      try {
        data = await api('GET', '/memos?status=' + memoView.status);
        S.memoPending = data.pending;
        const el = document.getElementById('memo-tab-count');
        if (el) { el.textContent = String(data.pending); el.hidden = false; el.classList.toggle('is-live', data.pending > 0); }
      } catch (e) { clear(list); list.appendChild(h('div', { class: 'empty' }, h('p', null, e.message))); return; }
      if (!list.isConnected) return;
      drawBanner();
      draw();
    };
    input.addEventListener('input', () => { memoView.q = input.value; draw(); });
    page.appendChild(h('div', { class: 'toolbar' },
      segmented([['pending', 'Waiting'], ['done', 'Done'], ['all', 'All']], memoView.status, 'Which reminders', (v) => { memoView.status = v; data = null; clear(list); list.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading…')); load(); }),
      h('label', { class: 'search-box' }, icon('search'), input), count));
    page.appendChild(list);
    drawBanner();
    load();
  }

  function memoRow(m, onCancel) {
    const name = (m.user && m.user.name) || (m.user ? 'Member ' + m.user.id : 'Unknown member');
    const [statusText, statusCls] = MEMO_STATUS[m.status] || [m.status, ''];
    const [viaText, viaIcon] = VIA[m.via] || [m.via, 'message'];
    const ch = m.channel || {};
    const place = ch.name ? '#' + ch.name + (ch.thread ? ' › thread' : '') : ch.thread ? 'a thread' : 'a DM or private channel';
    let whenEl;
    if (m.status === 'pending') whenEl = [h('b', null, m.due_words), h('small', { title: fmtFull.format(new Date(m.due_ts * 1000)) + ' IST' }, fromNow(m.due_ts))];
    else if (m.status === 'sent') whenEl = [h('b', null, 'Sent ' + ago(m.sent_ts || m.due_ts)), h('small', { title: fmtFull.format(new Date(m.due_ts * 1000)) + ' IST' }, 'was due ' + when(m.due_ts))];
    else whenEl = [h('b', null, m.status === 'failed' ? 'Couldn’t post' : 'Cancelled'), h('small', { title: fmtFull.format(new Date(m.due_ts * 1000)) + ' IST' }, 'was due ' + when(m.due_ts))];
    return h('div', { class: 'memo-row is-' + m.status },
      m.user ? h('a', { href: '#/members/' + m.user.id, class: 'memo-avatar', tabindex: '-1', 'aria-hidden': 'true' }, avatar(m.user.avatar, name, 'lg')) : h('span', { class: 'memo-avatar' }, avatar(null, '?', 'lg')),
      h('div', { class: 'memo-main' },
        h('div', { class: 'memo-who' }, m.user ? h('a', { href: '#/members/' + m.user.id }, name) : h('b', null, name),
          m.set_by ? h('span', { class: 'memo-by' }, 'set by ', h('span', { class: 'inline-ref' }, avatar(m.set_by.avatar, m.set_by.name || '?', 'xs'), '@' + (m.set_by.name || m.set_by.id))) : null),
        m.private ? h('p', { class: 'memo-text private' }, icon('shield'), ' Set somewhere private, so the text isn’t shown here.') : h('p', { class: 'memo-text' }, m.text),
        h('div', { class: 'memo-meta' },
          h('span', { class: 'badge ' + statusCls }, m.status === 'pending' ? h('span', { class: 'dot' }) : null, statusText),
          h('span', { class: 'memo-fact' }, icon(ch.name ? 'hash' : 'shield'), place),
          h('span', { class: 'memo-fact' }, icon(viaIcon), viaText),
          h('span', { class: 'memo-fact hide-sm', title: fmtFull.format(new Date(m.created_ts * 1000)) + ' IST' }, 'made ' + ago(m.created_ts)))),
      h('div', { class: 'memo-when' }, whenEl),
      h('div', { class: 'memo-actions' }, m.status === 'pending' ? h('button', { class: 'btn sm', type: 'button', onclick: onCancel, 'aria-label': 'Cancel ' + name + '’s reminder' }, icon('x'), 'Cancel') : null));
  }

  function openMemoForm(onSaved) {
    const general = S.channels.find((c) => c.kind === 'text' && c.name === 'general');
    const f = { user_id: '', channel_id: general ? general.id : '', when: '', text: '' };
    let member = null;
    let parsed = null;
    let seq = 0;
    const dr = openDrawer({
      title: 'New member reminder', sub: 'Loduchand pings the member in the channel when it’s due.', saveLabel: 'Set reminder',
      isDirty: () => !!(f.user_id || f.when.trim() || f.text.trim()),
      onSave: async (btn) => {
        if (!f.user_id) { toast('Pick the member to remind.', 'error'); return; }
        if (!f.channel_id) { toast('Pick the channel the reminder goes to.', 'error'); return; }
        if (!f.when.trim()) { toast('Say when, like “in 2 hours” or “tomorrow 9am”.', 'error'); return; }
        if (!f.text.trim()) { toast('What should the reminder say?', 'error'); return; }
        btn.disabled = true;
        try {
          const saved = await api('POST', '/memos', f);
          dr.finish();
          toast('Reminder set for ' + (saved.user && saved.user.name || 'the member') + ', ' + saved.due_words);
          refreshAudit();
          onSaved && onSaved(saved);
        } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
      },
    });
    const body = dr.body;
    const field = (label, control, hint, id) => h('div', null, h('label', { class: 'label', for: id || null }, label), control, hint || null);
    const whenNote = h('div', { class: 'hint when-note', 'aria-live': 'polite' }, 'India time. Like “in 2 hours”, “at 9pm”, “tomorrow 9am” or “20/09 18:00”.');
    const preview = h('div', { class: 'preview' });
    const counter = h('div', { class: 'line-count' });
    const drawPreview = () => {
      clear(preview);
      const who = h('span', { class: 'mention' }, '@' + (member ? member.name : 'member'));
      const text = f.text.trim() || 'what to remember';
      const me = S.me ? S.me.name : 'admin';
      const line = f.user_id && f.user_id !== (S.me && S.me.id) ? ['⏰ ', who, ' reminder from ', h('span', { class: 'mention' }, '@' + me), ': ', text] : ['⏰ ', who, ' reminder: ', text];
      preview.appendChild(postMessage({ text: line, time: parsed && parsed.ok ? parsed.words.replace(/^today at /, 'Today at ').replace(/^tomorrow at /, 'Tomorrow at ').replace(/ IST$/, '') : 'When it’s due' }));
    };
    const check = async () => {
      const mine = ++seq;
      const text = f.when.trim();
      if (!text) { parsed = null; whenNote.className = 'hint when-note'; whenNote.textContent = 'India time. Like “in 2 hours”, “at 9pm”, “tomorrow 9am” or “20/09 18:00”.'; drawPreview(); return; }
      try {
        const res = await api('GET', '/memos/when?text=' + encodeURIComponent(text));
        if (mine !== seq) return;
        parsed = res;
        clear(whenNote);
        if (res.ok) { whenNote.className = 'when-note ok'; append(whenNote, [icon('right'), h('b', null, res.words), h('span', null, ' · ' + fromNow(res.due_ts))]); }
        else { whenNote.className = 'error-text when-note'; append(whenNote, [icon('alert'), res.error]); }
        drawPreview();
      } catch (_) { /* keep the last answer */ }
    };
    let timer = null;
    const draw = () => {
      clear(body);
      const mBtn = h('button', { class: 'picker-btn', type: 'button', id: 'm-member', 'aria-haspopup': 'listbox' },
        member ? avatar(member.avatar, member.name, 'xs') : icon('user'), h('span', { class: 'value' + (member ? '' : ' placeholder') }, member ? member.name : 'Choose a member'), icon('chevron'));
      mBtn.addEventListener('click', () => openPicker(mBtn, { title: 'Member', placeholder: 'Search members by name', debounce: 180, load: memberItems, onPick: (it) => { f.user_id = it.id; member = it.member; draw(); } }));
      const c = chan(f.channel_id);
      const chBtn = h('button', { class: 'picker-btn', type: 'button', id: 'm-channel', 'aria-haspopup': 'listbox' }, h('span', { class: 'glyph' }, '#'),
        h('span', { class: 'value' + (c ? '' : ' placeholder') }, c ? c.name : 'Choose a channel'), c && c.category ? h('small', { style: 'color:var(--faint)' }, c.category) : null, icon('chevron'));
      chBtn.addEventListener('click', () => openPicker(chBtn, { title: 'Channel', placeholder: 'Search channels', load: channelItems('text', [f.channel_id]), onPick: (it) => { f.channel_id = it.id; draw(); } }));
      const when = h('input', { class: 'input', id: 'm-when', value: f.when, placeholder: 'e.g. at 9pm', autocomplete: 'off', maxlength: '80' });
      when.addEventListener('input', () => { f.when = when.value; clearTimeout(timer); timer = setTimeout(check, 220); });
      const quick = h('div', { class: 'quick-when' }, ['in 30m', 'in 2 hours', 'at 9pm', 'tomorrow 9am'].map((q) => h('button', { class: 'ph', type: 'button', onclick: () => { f.when = q; when.value = q; check(); } }, q)));
      const text = h('textarea', { class: 'textarea', id: 'm-text', rows: '3', maxlength: '400', placeholder: 'e.g. join quiz night in #quiz' });
      text.value = f.text;
      const count = () => { counter.textContent = f.text.length > 300 ? f.text.length + ' / 400' : ''; counter.classList.toggle('over', f.text.length > 400); };
      text.addEventListener('input', () => { f.text = text.value; count(); drawPreview(); });
      count();
      append(body, [
        h('section', { class: 'form-card' }, h('h3', null, icon('bell'), 'Reminder'),
          h('div', { class: 'form-grid' }, field('Member', mBtn, null, 'm-member'), field('Posts in', chBtn, null, 'm-channel')),
          field('When', h('div', null, when, quick), whenNote, 'm-when'),
          field('What to remind them', h('div', null, text, counter), null, 'm-text')),
        h('section', { class: 'form-card' }, h('h3', null, icon('eye'), 'Preview'), preview,
          h('p', { class: 'hint', style: 'margin:0' }, 'Only the member is pinged. They can see and cancel it with ', h('code', null, '/reminders'), '.')),
      ]);
      drawPreview();
    };
    draw();
    requestAnimationFrame(() => { const b = body.querySelector('#m-member'); if (b) b.focus({ preventScroll: true }); });
  }

  async function openReminderEditor(which, q) {
    const existing = which === 'new' ? null : S.reminders.find((r) => String(r.id) === String(which));
    if (which !== 'new' && !existing) { toast('That reminder no longer exists', 'error'); history.replaceState(null, '', '#/reminders'); currentHash = '#/reminders'; return; }
    let start = blankReminder();
    const tplKey = !existing && q ? q.get('t') : null;
    if (tplKey) {
      if (!S.templates) { try { S.templates = await api('GET', '/reminders/templates'); } catch (_) { S.templates = []; } }
      const tpl = S.templates.find((t) => t.key === tplKey);
      if (tpl) start = Object.assign(start, JSON.parse(JSON.stringify(tpl.reminder)), { channel_id: start.channel_id });
      if (!location.hash.startsWith('#/reminders/new')) return;
    }
    const d = Object.assign(blankReminder(), JSON.parse(JSON.stringify(existing || start)));
    if (!d.lines.length) d.lines = [''];
    let aiOn = !!d.ai_prompt;
    let aiDraft = d.ai_prompt;
    let aiSample = null;
    let reroll = 0;
    let testing = false;
    const original = JSON.stringify(d);
    let member = d.user_id ? S.members.get(d.user_id) || null : null;
    let previewIndex = d.lines.length ? d.sent_count % d.lines.length : 0;
    let everyUnit = d.schedule.kind === 'every' ? (d.schedule.minutes % 1440 === 0 ? 'days' : d.schedule.minutes % 60 === 0 ? 'hours' : 'minutes') : 'hours';
    const before = document.activeElement;

    const titleId = 'drawer-title';
    const body = h('div', { class: 'drawer-body' });
    const saveBtn = h('button', { class: 'btn primary', type: 'button' }, existing ? 'Save changes' : 'Create post');
    const drawer = h('div', { class: 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': titleId },
      h('div', { class: 'drawer-head' }, h('div', { class: 'grow' }, h('h2', { id: titleId }, existing ? 'Edit scheduled post' : tplKey ? 'New post from a template' : 'New scheduled post'),
        h('div', { class: 'sub' }, existing ? (existing.sent_count ? 'Sent ' + plural(existing.sent_count, 'time') + (existing.last_sent ? ', last ' + ago(existing.last_sent) : '') : 'Not sent yet') : 'Posts on its own once saved and switched on')),
        h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Close', onclick: () => attemptClose() }, icon('x'))),
      body,
      h('div', { class: 'drawer-foot' },
        existing ? h('button', { class: 'btn danger', type: 'button', onclick: remove, 'aria-label': 'Delete' }, icon('trash'), h('span', { class: 'hide-sm' }, 'Delete')) : null,
        existing ? h('button', { class: 'btn', type: 'button', onclick: sendTest }, icon('send'), h('span', { class: 'hide-sm' }, 'Send a test now'), h('span', { class: 'show-sm' }, 'Test')) : null,
        h('span', { class: 'grow' }),
        h('button', { class: 'btn ghost hide-sm', type: 'button', onclick: () => attemptClose() }, 'Cancel'),
        saveBtn));
    const scrim = h('div', { class: 'scrim', onclick: () => attemptClose() });
    const layer = pushLayer({ drawer: true, close: () => finish(true), attempt: () => attemptClose() });
    async function sendTest() {
      if (testing) return;
      const c = chan(existing.channel_id);
      const dirty = JSON.stringify(d) !== original;
      const ok = await confirmDialog({ title: 'Send a test post now?', icon: 'send', confirm: 'Send test',
        body: h('div', null,
          h('p', null, 'Loduchand posts “' + existing.name + '” once in ', h('b', null, c ? '#' + c.name : 'its channel'), ', exactly as it would go out' + (existing.ai_prompt ? ', with a fresh AI-written text' : '') + '. Everyone in the channel sees it.'),
          h('p', null, 'It doesn’t count as a scheduled post and doesn’t change when the next one goes out.'),
          dirty ? h('p', { class: 'error-text' }, icon('alert'), 'Your unsaved changes aren’t in it. Save first to test them.') : null) });
      if (!ok) return;
      testing = true;
      try { await api('POST', '/reminders/' + existing.id + '/test'); toast('Test posted in ' + (c ? '#' + c.name : 'the channel')); }
      catch (e) { toast(e.message, 'error'); }
      testing = false;
    }
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
      if (!d.lines.some((l) => l.trim()) && !d.ai_prompt.trim() && !d.images.length) return 'Add at least one message line.';
      if (d.colour && !/^#[0-9a-f]{6}$/i.test(d.colour)) return 'The card colour should look like #8b93ff.';
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
      const time = next ? (next.day === 'today' ? 'Today at ' : next.day === 'tomorrow' ? 'Tomorrow at ' : '') + next.time : 'Today';
      const imgs = d.images || [];
      const imgId = !imgs.length ? null : d.image_order === 'same' ? imgs[0] : d.image_order === 'random' ? imgs[reroll % imgs.length] : imgs[(d.sent_count + (lines.length ? idx : 0)) % imgs.length];
      const image = imgId ? mediaImg(imgId, 'post-img', (mediaById(imgId) || {}).name || 'picture') : null;
      const useAi = aiOn && d.ai_prompt.trim();
      const at = next ? next.at : null;
      const text = useAi && aiSample ? aiSample : lines.length ? fillLine(fillExtras(lines[idx], reroll, at), d, member) : null;
      const chName = chan(d.channel_id);
      const hasRandom = /\{random:/.test(d.lines.join(' ') + d.title + d.footer) || (d.image_order === 'random' && imgs.length > 1);
      append(preview, [
        h('div', { class: 'preview-tools' }, h('span', null, chName ? '#' + chName.name : 'No channel yet'), useAi ? h('span', { class: 'badge kind' }, icon('spark'), aiSample ? 'AI sample' : 'Fallback line') : null, h('span', { class: 'grow' }),
          hasRandom ? h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { reroll++; drawPreview(); } }, icon('shuffle'), 'Another pick') : null,
          lines.length > 1 && !(useAi && aiSample) ? [
            h('span', null, (d.order === 'random' ? 'Random · ' : 'Line ') + (idx + 1) + ' of ' + lines.length),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Previous line', onclick: () => { previewIndex = (idx - 1 + lines.length) % lines.length; drawPreview(); } }, icon('up')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Next line', onclick: () => { previewIndex = (idx + 1) % lines.length; drawPreview(); } }, icon('down')),
          ] : null),
        h('div', { class: 'preview' },
          text || image || (d.style === 'card' && d.title) ? postMessage({ style: d.style, colour: d.colour, title: d.style === 'card' ? fillExtras(d.title, reroll, at) : '', footer: d.style === 'card' ? fillExtras(d.footer, reroll, at) : '', text, image, reactions: d.reactions, time })
            : h('p', { style: 'color:#949ba4' }, 'Write a line to see it here.'),
          d.stop_when_back && d.welcome_line.trim() ? postMessage({ text: fillLine(d.welcome_line, d, member), time: 'When they’re back' }) : null),
        useAi && !aiSample ? h('p', { class: 'hint', style: 'margin:0' }, 'The AI writes each post. Press “Try it” under Extras to see a sample; the line above is what goes out if the AI can’t answer.') : null,
        d.style === 'card' && /\{mention\}/.test(d.lines.join(' ')) ? h('p', { class: 'error-text', style: 'margin:0' }, icon('alert'), 'Mentions inside a card don’t ping anyone. Use plain text to ping the member.') : null,
        h('p', { class: 'next-note' }, icon('clock'), next ? 'Next post around ' + next.time + ' IST ' + next.day + (d.enabled ? '' : ' once switched on') + '.' : 'No post is due in the next week with these settings.'),
      ]);
    };

    let lastFocused = null;
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
      d.lines.forEach((line, i) => {
        const ta = h('textarea', { class: 'textarea', rows: '1', 'aria-label': 'Line ' + (i + 1), placeholder: i === 0 ? 'e.g. {mention}, it has been {days} days!' : 'Another line', maxlength: '2000' });
        ta.value = line;
        const counter = h('div', { class: 'line-count' + (line.length > 1800 ? ' over' : ''), 'aria-live': 'polite' }, line.length > 1500 ? line.length + ' / 1800' : '');
        const grow = () => { ta.style.height = 'auto'; ta.style.height = Math.min(220, ta.scrollHeight + 2) + 'px'; };
        ta.addEventListener('input', () => { d.lines[i] = ta.value; grow(); counter.textContent = ta.value.length > 1500 ? ta.value.length + ' / 1800' : ''; counter.classList.toggle('over', ta.value.length > 1800); previewIndex = i; drawPreview(); });
        ta.addEventListener('focus', () => { lastFocused = ta; });
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
        if (ta && ta.id === 'r-ai' && /^\{(name|mention|hours|days)\}$/.test(ph)) { toast(ph + ' works in lines, not the AI prompt.', 'error'); return; }
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
          h('div', { class: 'placeholders' }, 'Insert:', PLACEHOLDERS.map(([ph, tip]) => h('button', { class: 'ph', type: 'button', onmousedown: (e) => e.preventDefault(), onclick: () => insert(ph === '{countdown}' ? '{countdown:' + addDays(istToday(), 7) + '}' : ph === '{random}' ? '{random:first|second|third}' : ph), 'data-tip': tip }, ph === '{countdown}' ? '{countdown:date}' : ph === '{random}' ? '{random:a|b}' : ph)))),
        aiOn && d.ai_prompt.trim() ? h('p', { class: 'hint', style: 'margin:0' }, 'The AI writes the posts. These lines go out only when it can’t.') : null));

      // picture
      const imgs = d.images;
      const strip = h('div', { class: 'pic-strip' });
      imgs.forEach((id, i) => {
        const m = mediaById(id);
        strip.appendChild(h('div', { class: 'pic' }, mediaImg(id, 'pic-img', m ? m.name : 'picture'),
          imgs.length > 1 ? h('span', { class: 'pic-n' }, String(i + 1)) : null,
          h('button', { class: 'pic-x', type: 'button', 'aria-label': 'Remove ' + (m ? m.name : 'picture ' + (i + 1)), onclick: () => { imgs.splice(i, 1); draw(); } }, icon('x'))));
      });
      const libBtn = h('button', { class: 'btn', type: 'button', onclick: () => openLibrary(imgs, () => draw()) }, icon('image'), 'Library');
      const zone = dropZone(imgs.length ? 'Add another picture' : 'Upload a picture', 'Drop PNG, JPG, GIF or WebP here, up to 8 MB', async (files) => {
        zone.classList.add('busy');
        const saved = await uploadPictures(files);
        saved.forEach((m) => { if (!imgs.includes(m.id)) imgs.push(m.id); });
        draw();
      });
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('image'), 'Picture', h('span', { class: 'right' }, imgs.length > 1 ? segmented([['rotate', 'In turn', 'repeat'], ['random', 'Random', 'shuffle'], ['same', 'Always #1']], d.image_order, 'Which picture', (v) => { d.image_order = v; draw(); }) : h('span', { style: 'color:var(--faint);font-size:12.5px' }, 'optional'))),
        imgs.length ? strip : null,
        h('div', { class: 'pic-tools' }, zone, libBtn),
        h('p', { class: 'hint', style: 'margin:0' }, imgs.length > 1 ? (d.image_order === 'same' ? 'The first picture goes with every post.' : d.image_order === 'random' ? 'One of these at random with each post.' : 'Each post takes the next picture in turn.') : imgs.length ? 'This picture goes with every post. Add more to take turns or pick at random.' : 'Attached to each post. Add a few and they take turns, or one is picked at random.')));

      // style
      const colourText = h('input', { class: 'input mono', id: 'r-colour', value: d.colour, placeholder: '#8b93ff', maxlength: '7', style: 'max-width:110px', 'aria-label': 'Card colour' });
      const colourPick = h('input', { type: 'color', class: 'colour-pick', value: /^#[0-9a-f]{6}$/i.test(d.colour) ? d.colour : '#8b93ff', 'aria-label': 'Pick the card colour' });
      colourPick.addEventListener('input', () => { d.colour = colourPick.value; colourText.value = d.colour; drawPreview(); });
      colourText.addEventListener('input', () => { d.colour = colourText.value.trim(); colourText.classList.toggle('invalid', !!d.colour && !/^#[0-9a-f]{6}$/i.test(d.colour)); if (/^#[0-9a-f]{6}$/i.test(d.colour)) colourPick.value = d.colour; drawPreview(); });
      const swatches = h('div', { class: 'swatches' }, [['#8b93ff', 'Loduchand'], ['#9b1b1b', 'Gryffindor'], ['#1a6b4a', 'Slytherin'], ['#1f4e8c', 'Ravenclaw'], ['#d6a318', 'Hufflepuff'], ['#e0a43a', 'Amber'], ['#3ba55d', 'Green'], ['#eb459e', 'Pink']]
        .map(([hex, label]) => h('button', { class: 'swatch-btn' + ((d.colour || '').toLowerCase() === hex ? ' is-on' : ''), type: 'button', style: 'background:' + hex, 'aria-label': label + ' ' + hex, 'data-tip': label, onclick: () => { d.colour = hex; draw(); } })));
      const title = h('input', { class: 'input', id: 'r-title', value: d.title, maxlength: '256', placeholder: 'e.g. 🏆 House Cup · {date}' });
      title.addEventListener('input', () => { d.title = title.value; drawPreview(); });
      title.addEventListener('focus', () => { lastFocused = title; });
      const footer = h('input', { class: 'input', id: 'r-footer', value: d.footer, maxlength: '300', placeholder: 'e.g. Points reset on the 1st' });
      footer.addEventListener('input', () => { d.footer = footer.value; drawPreview(); });
      footer.addEventListener('focus', () => { lastFocused = footer; });
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('table'), 'Style', h('span', { class: 'right' }, segmented([['plain', 'Plain text', 'message'], ['card', 'Card', 'table']], d.style, 'Post style', (v) => { d.style = v; draw(); }))),
        d.style === 'card' ? h('div', { class: 'form-grid' },
          field('Title', title, 'Placeholders work here too.', { full: true, optional: true }),
          field('Colour', h('div', { class: 'colour-row' }, colourPick, colourText, swatches), null, { full: true }),
          field('Footer', footer, null, { full: true, optional: true }))
          : h('p', { class: 'hint', style: 'margin:0' }, 'The line as an ordinary message, with the picture under it. A card puts it in a coloured box with a title and a footer.')));

      // extras
      const reactions = h('div', { class: 'chip-input' });
      const rIn = h('input', { type: 'text', id: 'r-reaction', placeholder: d.reactions.length ? 'Add…' : 'Paste an emoji, then Enter', 'aria-label': 'Add a reaction', autocomplete: 'off' });
      const addReaction = () => { rIn.value.split(',').map((x) => x.trim()).filter(Boolean).forEach((x) => { if (!d.reactions.includes(x) && d.reactions.length < 5) d.reactions.push(x); }); rIn.value = ''; draw(); const again = body.querySelector('#r-reaction'); if (again) again.focus(); };
      d.reactions.forEach((t, i) => reactions.appendChild(h('span', { class: 'chip' }, emojiEl(t), h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + t, onclick: () => { d.reactions.splice(i, 1); draw(); } }, icon('x')))));
      rIn.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addReaction(); } else if (e.key === 'Backspace' && !rIn.value && d.reactions.length) { d.reactions.pop(); draw(); const again = body.querySelector('#r-reaction'); if (again) again.focus(); } });
      rIn.addEventListener('blur', () => { if (rIn.value.trim()) addReaction(); });
      if (d.reactions.length < 5) reactions.appendChild(rIn);
      reactions.addEventListener('click', (e) => { if (e.target === reactions) rIn.focus(); });
      const serverEmoji = h('button', { class: 'btn sm', type: 'button', disabled: d.reactions.length >= 5 }, icon('smile'), 'Server emoji');
      serverEmoji.addEventListener('click', async () => {
        const list = await loadEmojis();
        openPicker(serverEmoji, { title: 'Server emoji', placeholder: 'Search the server’s emoji', empty: 'This server has no custom emoji.',
          load: (q) => list.filter((e) => !q || e.name.toLowerCase().includes(q.toLowerCase())).map((e) => ({ id: e.id, label: ':' + e.name + ':', sub: e.animated ? 'animated' : '', lead: h('img', { class: 'emoji-img', src: e.url, alt: '' }), emoji: e })),
          onPick: (it) => { const code = '<' + (it.emoji.animated ? 'a' : '') + ':' + it.emoji.name + ':' + it.emoji.id + '>'; if (!d.reactions.includes(code) && d.reactions.length < 5) d.reactions.push(code); draw(); } });
      });
      const tidy = h('input', { type: 'checkbox', checked: d.delete_previous });
      tidy.addEventListener('change', () => { d.delete_previous = tidy.checked; });
      const aiBox = h('input', { type: 'checkbox', checked: aiOn });
      aiBox.addEventListener('change', () => { aiOn = aiBox.checked; if (aiOn) d.ai_prompt = aiDraft || ''; else { aiDraft = d.ai_prompt; d.ai_prompt = ''; aiSample = null; } draw(); if (aiOn) { const t = body.querySelector('#r-ai'); if (t) t.focus(); } });
      const aiText = h('textarea', { class: 'textarea', id: 'r-ai', rows: '3', maxlength: '1000', placeholder: 'e.g. A fun, easy question for everyone: this-or-that, food, films or cricket. One question only.' });
      aiText.value = d.ai_prompt;
      aiText.addEventListener('input', () => { d.ai_prompt = aiText.value; aiDraft = aiText.value; });
      aiText.addEventListener('focus', () => { lastFocused = aiText; });
      const aiOut = h('div', { class: 'ai-out', 'aria-live': 'polite' }, aiSample ? h('small', null, 'The preview shows the AI’s sample.') : h('small', null, '{date}, {leader}, {standings} and the rest are filled in before the AI reads it.'));
      const tryBtn = h('button', { class: 'btn sm', type: 'button' }, icon('spark'), 'Try it');
      tryBtn.addEventListener('click', async () => {
        if (!d.ai_prompt.trim()) { toast('Write what the AI should post first.', 'error'); aiText.focus(); return; }
        tryBtn.disabled = true;
        clear(aiOut); aiOut.appendChild(h('span', { class: 'ai-wait' }, h('span', { class: 'spinner' }), 'Asking the AI…'));
        try {
          const res = await api('POST', '/reminders/ai-preview', { prompt: d.ai_prompt });
          aiSample = res.text;
          clear(aiOut); aiOut.appendChild(h('small', null, 'Sample from ' + (res.model || 'the model') + ', shown in the preview. Each real post is written fresh.'));
          drawPreview();
          const pv = body.querySelector('.preview'); if (pv) pv.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        } catch (e) { clear(aiOut); aiOut.appendChild(h('span', { class: 'error-text', style: 'margin:0' }, icon('alert'), e.message)); }
        tryBtn.disabled = false;
      });
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('zap'), 'Extras', h('span', { class: 'right', style: 'color:var(--faint);font-size:12.5px' }, 'optional')),
        h('div', null, h('label', { class: 'label', for: 'r-reaction' }, 'React to its own post with'), reactions, h('div', { class: 'hint-row' }, h('span', { class: 'hint', style: 'margin:0' }, 'Up to 5. Unicode emoji, or the server’s own.'), serverEmoji)),
        h('label', { class: 'check' }, tidy, h('span', null, 'Delete the previous post when a new one goes out', h('small', null, 'Keeps the channel tidy: only the latest one stays. Test posts are never deleted.'))),
        h('div', { class: 'ai-block' + (aiOn ? ' is-on' : '') },
          h('label', { class: 'check' }, aiBox, h('span', null, h('span', { class: 'ai-label' }, icon('spark'), 'Let the AI write each post'), h('small', null, 'It follows your prompt, remembers its last 5 posts so it doesn’t repeat itself, and stays under 400 characters with no pings. If it can’t answer, a line from Messages goes out.'))),
          aiOn ? h('div', { class: 'ai-edit' }, aiText, h('div', { class: 'hint-row' }, aiOut, tryBtn)) : null)));

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
    Promise.all([loadPlaceholders(), loadMedia(), d.reactions.length ? loadEmojis() : null]).then(() => { if (body.isConnected) draw(); });
  }

  // --- special welcomes ------------------------------------------------------------------

  const WELCOME_PLACEHOLDERS = [
    ['{mention}', 'Mentions them (pings them when “Ping them” is on)'], ['{name}', 'Their name'], ['{n}', 'Which join this is, like 3rd'],
    ['{away}', 'How long they were gone, like 7 days'], ['{days}', 'Days since they left'], ['{hours}', 'Hours since they left'],
  ];
  const WELCOME_FIELDS = ['user_id', 'user_name', 'channel_id', 'lines', 'also_ping', 'ping_member', 'mode', 'replace_normal', 'enabled', 'note'];
  /** "12 Sep", built by hand: browsers disagree on "Sep" and "Sept". */
  const dayMonth = (ts) => istParts(ts * 1000, { day: 'numeric' }) + ' ' + ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][+istParts(ts * 1000, { month: 'numeric' }) - 1];

  async function loadWelcomes() {
    try { S.welcomes = await api('GET', '/welcomes'); } catch (e) { if (!S.welcomes) throw e; }
    return S.welcomes;
  }

  function welcomeName(w) { return (w.member && w.member.name) || w.user_name || 'Member ' + w.user_id; }

  /** [text, badge class, live dot]. */
  function welcomeStatus(w) {
    const last = w.last_fired_ts ? dayMonth(w.last_fired_ts) : '';
    if (w.enabled && w.fired_count) return ['Welcomed ' + last, 'on', true];
    if (w.enabled) return ['Waiting for them', 'waiting', true];
    if (w.fired_count) return ['Welcomed ' + last, 'on', false];
    return ['Off', 'paused', false];
  }

  /** Names for the `<@id>` in a welcome: the member and whoever it also pings. */
  function welcomeNames(w) {
    const names = {};
    (w.also || []).forEach((p) => { if (p.name) names[p.id] = p.name; });
    names[w.user_id] = welcomeName(w);
    return names;
  }

  /** A line as written: placeholders marked, `<@id>` as the person's name. */
  function welcomeTemplated(line, names) {
    return String(line).replace(/\*\*/g, '').split(/(\{(?:mention|name|n|away|days|hours)\}|<@!?\d+>)/g).map((part, i) => {
      if (!(i % 2)) return part;
      const m = /^<@!?(\d+)>$/.exec(part);
      return m ? h('span', { class: 'quote-mention' }, '@' + (names[m[1]] || 'someone')) : h('span', { class: 'ph-inline' }, part);
    });
  }

  /** Posted text as Discord shows it: mentions by name, **bold**. */
  function mentionParts(text, names) {
    return discordMarkup(String(text).split(/(<@!?\d+>)/g).map((part, i) => {
      if (!(i % 2)) return part;
      const id = /\d+/.exec(part)[0];
      return h('span', { class: 'mention' }, '@' + (names[id] || 'unknown-user'));
    }));
  }

  function renderWelcomes(page) {
    document.title = 'Welcomes · Loduchand';
    page.appendChild(pageHead('Welcomes', 'A message of your own that Loduchand posts the moment a particular member joins the server. Use it for someone you’re waiting on to come back.',
      h('button', { class: 'btn primary', type: 'button', onclick: () => navigate('#/welcomes/new') }, icon('plus'), 'New welcome')));
    const banner = h('div');
    const grid = h('div', { class: 'reminders welcomes' });
    page.appendChild(banner);
    page.appendChild(grid);
    const draw = () => {
      clear(banner);
      clear(grid);
      const data = S.welcomes;
      if (!data) { grid.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading welcomes…')); return; }
      const link = h('a', { href: '#/s/welcome?k=VIZIER_SPECIAL_WELCOMES' }, 'Special welcomes');
      if (!data.enabled) banner.appendChild(h('div', { class: 'memo-note is-off' }, icon('pause'), h('p', null, h('b', null, 'Special welcomes are switched off, so none of these post. '), 'Switch ', link, ' back on under Welcome & join history.')));
      else if (!data.welcome_channel) banner.appendChild(h('div', { class: 'memo-note is-off' }, icon('alert'), h('p', null, h('b', null, 'No welcome channel is set. '), 'Each welcome needs its own channel until one is.')));
      if (!data.items.length) {
        grid.appendChild(h('div', { class: 'welcome-empty' },
          h('span', { class: 'welcome-empty-icon', 'aria-hidden': 'true' }, '👋'),
          h('h3', null, 'No special welcomes yet'),
          h('p', null, 'Waiting on someone to come back? Write them a welcome here. Loduchand posts it the moment they join, pings the people who missed them, and switches it off once it’s gone out. It works for first-timers too.'),
          h('button', { class: 'btn primary', type: 'button', onclick: () => navigate('#/welcomes/new') }, icon('plus'), 'New welcome')));
        return;
      }
      data.items.forEach((w) => grid.appendChild(welcomeCard(w, data)));
      if (data.items.length < data.max) grid.appendChild(h('button', { class: 'new-card', type: 'button', onclick: () => navigate('#/welcomes/new') }, icon('plus'), 'New welcome', h('small', null, 'For someone you’re waiting on')));
    };
    draw();
    loadWelcomes().then(() => { if (grid.isConnected) { draw(); renderSidebar(); } }).catch((e) => { if (grid.isConnected) { clear(grid); grid.appendChild(h('div', { class: 'empty' }, h('p', null, e.message))); } });
  }

  function welcomeCard(w, data) {
    const name = welcomeName(w);
    const m = w.member || {};
    const [statusText, statusCls, live] = welcomeStatus(w);
    const names = welcomeNames(w);
    const el = h('article', { class: 'reminder welcome-card' + (w.enabled ? '' : ' is-off'), 'aria-label': 'Welcome for ' + name });
    el.appendChild(h('div', { class: 'reminder-head' },
      h('a', { class: 'welcome-avatar', href: m.in_server ? '#/members/' + w.user_id : '#/welcomes/' + w.id, tabindex: '-1', 'aria-hidden': 'true' }, avatar(m.avatar, name, 'lg')),
      h('div', { class: 'grow' }, h('h3', null, name, h('span', { class: 'welcome-id mono', title: 'Discord ID' }, w.user_id)),
        h('div', { class: 'sub' },
          h('span', { class: 'badge ' + statusCls }, live ? h('span', { class: 'dot' }) : null, statusText),
          h('span', { class: 'badge kind' }, icon(w.mode === 'every' ? 'repeat' : 'check'), w.mode === 'every' ? 'Every time' : 'Once')))));
    const facts = h('ul', { class: 'reminder-facts' });
    const ch = w.channel;
    facts.appendChild(h('li', null, icon('hash'), ch ? h('span', null, channelRef(ch.id, { bare: true }), ch.default ? h('span', { style: 'color:var(--faint)' }, ' · welcome channel') : null)
      : h('span', { class: 'warn-text' }, 'No channel: set a welcome channel')));
    const also = w.also || [];
    facts.appendChild(h('li', null, icon('bell'), h('span', null,
      also.length ? h('span', { class: 'avatar-stack' }, also.slice(0, 4).map((p) => avatar(p.avatar, p.name || p.id, 'xs'))) : null,
      w.ping_member ? 'Pings ' + (also.length ? 'them' : 'only them') : 'Doesn’t ping them',
      also.length ? (w.ping_member ? ' and ' : ' · pings ') + also.slice(0, 2).map((p) => p.name || 'someone').join(', ') + (also.length > 2 ? ' +' + (also.length - 2) : '') : '')));
    facts.appendChild(h('li', null, icon('message'), h('span', null, w.replace_normal ? 'Instead of the usual welcome' : 'Under the usual welcome')));
    if (w.fired_count) facts.appendChild(h('li', null, icon('clock'), h('span', { title: fmtFull.format(new Date(w.last_fired_ts * 1000)) + ' IST' }, 'Welcomed ' + plural(w.fired_count, 'time') + ' · last ' + ago(w.last_fired_ts))));
    else if (w.enabled && m.in_server) facts.appendChild(h('li', null, icon('user'), h('span', null, 'In the server now · posts when they next join')));
    el.appendChild(facts);
    if (w.lines[0]) el.appendChild(h('p', { class: 'reminder-quote', title: w.lines[0] }, welcomeTemplated(w.lines[0].split('\n')[0], names)));
    if (w.note) el.appendChild(h('p', { class: 'welcome-note', title: w.note }, w.note));
    el.appendChild(h('div', { class: 'reminder-foot' },
      h('span', { class: 'grow' }, plural(w.lines.length, 'version') + ' · added ' + ago(w.created_ts)),
      h('a', { class: 'btn sm', href: '#/welcomes/' + w.id }, icon('edit'), 'Edit')));
    return el;
  }

  async function openWelcomeEditor(which) {
    if (!S.welcomes) { try { await loadWelcomes(); } catch (e) { toast(e.message, 'error'); return; } }
    if (!location.hash.startsWith('#/welcomes/')) return;
    const data = S.welcomes;
    const existing = which === 'new' ? null : data.items.find((w) => String(w.id) === String(which));
    if (which !== 'new' && !existing) { toast('That welcome no longer exists', 'error'); history.replaceState(null, '', '#/welcomes'); currentHash = '#/welcomes'; return; }
    const d = { user_id: '', user_name: '', channel_id: '', lines: [''], also_ping: [], ping_member: true, mode: 'once', replace_normal: true, enabled: true, note: '' };
    if (existing) WELCOME_FIELDS.forEach((k) => { d[k] = JSON.parse(JSON.stringify(existing[k])); });
    if (!d.lines.length) d.lines = [''];
    const original = JSON.stringify(d);
    // Who it's for: { id, name, username, avatar, in_server, found }, or a lookup in flight.
    let person = existing ? Object.assign({ found: !!existing.member.known }, existing.member) : null;
    let looking = false;
    let lookupError = '';
    let idDraft = '';
    const people = new Map();
    if (existing) (existing.also || []).forEach((p) => people.set(p.id, p));
    let result = null;
    let previewIndex = 0;
    let testing = false;
    let lastFocused = null;
    let seq = 0;

    const dirty = () => JSON.stringify(d) !== original;
    const sub = existing
      ? (existing.fired_count ? 'Welcomed ' + plural(existing.fired_count, 'time') + ', last ' + ago(existing.last_fired_ts) + (existing.enabled ? '' : ' · switch it on to welcome them again') : existing.enabled ? 'Waiting for ' + welcomeName(existing) + ' to join' : 'Switched off')
      : 'Posts the moment this member joins the server';
    const testBtn = existing ? h('button', { class: 'btn', type: 'button', onclick: () => sendTest() }, icon('send'), h('span', { class: 'hide-sm' }, 'Send a test now'), h('span', { class: 'show-sm' }, 'Test')) : null;
    const dr = openDrawer({
      title: existing ? 'Welcome for ' + welcomeName(existing) : 'New welcome', sub, returnHash: '#/welcomes',
      saveLabel: existing ? 'Save changes' : 'Create welcome', isDirty: dirty, footExtra: testBtn,
      onDelete: existing ? () => remove() : null,
      onSave: async (btn) => {
        const problem = localCheck();
        if (problem) { toast(problem, 'error'); return; }
        btn.disabled = true;
        const payload = Object.assign({}, d, { lines: d.lines.filter((l) => l.trim()) });
        try {
          const saved = existing ? await api('PUT', '/welcomes/' + existing.id, payload) : await api('POST', '/welcomes', payload);
          dr.finish();
          await loadWelcomes().catch(() => null);
          rerender();
          toast(existing ? 'Saved the welcome for ' + welcomeName(saved) : 'Welcome for ' + welcomeName(saved) + ' is ' + (saved.enabled ? 'waiting for them' : 'saved, switched off'));
          refreshAudit();
        } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
      },
    });
    const body = dr.body;
    body.classList.add('welcome-editor');

    function localCheck() {
      if (!d.user_id) return 'Pick the member to welcome, or paste their Discord ID.';
      if (person && !person.found && !d.user_name.trim()) return 'Type a name for them: Discord doesn’t know this ID.';
      if (!d.lines.some((l) => l.trim())) return 'Write the message to post when they join.';
      if (d.lines.some((l) => l.length > 1500)) return 'A message is longer than 1500 characters.';
      if (!d.channel_id && !data.welcome_channel) return 'Pick a channel: no welcome channel is set.';
      return null;
    }

    async function remove() {
      const ok = await confirmDialog({ title: 'Delete the welcome for ' + welcomeName(existing) + '?', icon: 'trash', danger: true, body: 'Nothing will be posted when they join, apart from the usual welcome. To keep it for later, switch it off instead.', confirm: 'Delete welcome' });
      if (!ok) return;
      try {
        await api('DELETE', '/welcomes/' + existing.id);
        dr.finish();
        await loadWelcomes().catch(() => null);
        rerender();
        toast('Deleted the welcome for ' + welcomeName(existing));
        refreshAudit();
      } catch (e) { toast(e.message, 'error'); }
    }

    async function sendTest() {
      if (testing) return;
      const c = existing.channel ? chan(existing.channel.id) : null;
      const ok = await confirmDialog({ title: 'Send a test now?', icon: 'send', confirm: 'Send test',
        body: h('div', null,
          h('p', null, 'Loduchand posts one of the versions in ', h('b', null, c ? '#' + c.name : 'its channel'), ', starting with “(test)”. ', h('b', null, 'Nobody is pinged'), ', not even the people it mentions.'),
          h('p', null, 'It doesn’t count as their welcome: it still waits for them to join.'),
          dirty() ? h('p', { class: 'error-text' }, icon('alert'), 'Your unsaved changes aren’t in it. Save first to test them.') : null) });
      if (!ok) return;
      testing = true;
      try { await api('POST', '/welcomes/' + existing.id + '/test'); toast('Test posted in ' + (c ? '#' + c.name : 'the channel')); }
      catch (e) { toast(e.message, 'error'); }
      testing = false;
    }

    // --- preview ---
    const preview = h('div', { class: 'welcome-preview' });
    const refresh = async () => {
      const mine = ++seq;
      const lines = d.lines.map((l) => l);
      try {
        const res = await api('POST', '/welcomes/preview', { user_id: d.user_id, user_name: d.user_name, lines, ping_member: d.ping_member, also_ping: d.also_ping });
        if (mine !== seq) return;
        result = res;
      } catch (_) { if (mine !== seq) return; result = null; }
      drawPreview();
    };
    let timer = null;
    const soon = () => { clearTimeout(timer); timer = setTimeout(refresh, 220); };
    const drawPreview = () => {
      clear(preview);
      const filled = d.lines.map((l, i) => ({ l, i })).filter((x) => x.l.trim());
      const at = filled.findIndex((x) => x.i === previewIndex);
      const idx = at >= 0 ? at : 0;
      const shown = filled[idx];
      const names = Object.assign({}, (result && result.names) || {});
      people.forEach((p, id) => { if (!names[id] && p.name) names[id] = p.name; });
      const who = (result && result.name) || d.user_name || (person && person.name) || 'them';
      if (d.user_id) names[d.user_id] = who;
      const c = d.channel_id ? chan(d.channel_id) : data.welcome_channel ? chan(data.welcome_channel.id) : null;
      const text = shown && result && typeof result.items[shown.i] === 'string' ? result.items[shown.i] : null;
      append(preview, [
        h('div', { class: 'preview-tools' }, h('span', null, c ? '#' + c.name : 'No channel yet'), h('span', { class: 'grow' }),
          filled.length > 1 ? [
            h('span', null, 'Version ' + (idx + 1) + ' of ' + filled.length),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Previous version', onclick: () => { previewIndex = filled[(idx - 1 + filled.length) % filled.length].i; drawPreview(); } }, icon('up')),
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Next version', onclick: () => { previewIndex = filled[(idx + 1) % filled.length].i; drawPreview(); } }, icon('down')),
          ] : null),
        h('div', { class: 'preview' },
          !d.replace_normal ? h('div', { class: 'msg-usual' }, postMessage({ text: ['Loduchand’s usual welcome line for ', h('span', { class: 'mention' }, '@' + who), ' goes here.'], time: 'When they join' })) : null,
          text !== null ? postMessage({ text: mentionParts(text, names), time: 'When they join' })
            : h('p', { style: 'color:#949ba4;margin:0' }, shown ? 'Filling it in…' : 'Write the message to see it here.')),
      ]);
      if (result) {
        const f = result.facts;
        preview.appendChild(h('p', { class: 'next-note' }, icon('info'), h('span', null, f.from_log
          ? ['If they joined now: ', h('code', null, '{n}'), ' is ' + f.n + ' and ', h('code', null, '{away}'), ' is ' + f.away + ', from their join history.']
          : ['No join history for them yet, so ', h('code', null, '{n}'), ' reads ' + f.n + ' and ', h('code', null, '{away}'), ' reads “' + f.away + '”.'])));
        const missing = (result.unpinged || []).filter((id) => id !== d.user_id);
        if (missing.length) {
          preview.appendChild(h('div', { class: 'welcome-warn' }, icon('alert'),
            h('span', null, missing.map((id) => '@' + (names[id] || id)).join(', ') + (missing.length === 1 ? ' is' : ' are') + ' mentioned but won’t be pinged.'),
            d.also_ping.length + missing.length <= 10 ? h('button', { class: 'btn sm', type: 'button', onclick: () => { missing.forEach((id) => { if (!d.also_ping.includes(id)) d.also_ping.push(id); }); draw(); } }, icon('plus'), 'Ping them too') : null));
        }
        if (d.user_id && !d.ping_member && (result.items || []).some((t) => t.includes('<@' + d.user_id + '>'))) {
          preview.appendChild(h('p', { class: 'hint', style: 'margin:0' }, 'Their mention shows their name without pinging them.'));
        }
      }
    };

    // --- who it's for ---
    const lookup = async (raw) => {
      const id = raw.trim();
      lookupError = '';
      if (!id) { draw(); return; }
      if (!/^\d{17,20}$/.test(id)) { lookupError = 'Discord IDs are 17 to 20 digits.'; draw(); return; }
      looking = true;
      d.user_id = id;
      person = null;
      draw();
      try {
        const found = await api('GET', '/welcomes/lookup?id=' + encodeURIComponent(id));
        if (d.user_id !== id) return;
        person = Object.assign({ found: true }, found);
        d.user_name = found.name;
      } catch (e) {
        if (d.user_id !== id) return;
        if (e.status === 404) { person = { found: false, id, name: null }; d.user_name = ''; }
        else { d.user_id = ''; lookupError = e.message; }
      }
      looking = false;
      draw();
      if (person && !person.found) { const n = body.querySelector('#w-name'); if (n) n.focus(); }
    };

    const field = (label, control, hint, id) => h('div', null, h('label', { class: 'label', for: id || null }, label), control, hint ? h('div', { class: 'hint' }, hint) : null);

    const draw = () => {
      const scroll = body.scrollTop;
      clear(body);

      // member
      const enabled = switchEl(d.enabled, 'Welcome on', (on, btn) => { d.enabled = on; btn.set(on); });
      const memberCard = h('section', { class: 'form-card' }, h('h3', null, icon('user'), 'Member', h('span', { class: 'right' }, enabled)));
      const clash = person && person.welcome_id && (!existing || person.welcome_id !== existing.id) ? data.items.find((w) => w.id === person.welcome_id) : null;
      if (d.user_id) {
        const p = person || { id: d.user_id };
        const tag = looking ? h('span', { class: 'badge' }, h('span', { class: 'spinner' }), 'Looking up…')
          : !p.found ? h('span', { class: 'badge failed' }, 'Unknown to Discord')
            : p.in_server === true ? h('span', { class: 'badge on' }, 'In the server') : p.in_server === false ? h('span', { class: 'badge waiting' }, 'Not in the server') : null;
        memberCard.appendChild(h('div', { class: 'person-row' },
          looking ? h('span', { class: 'avatar lg' }, h('span', { class: 'spinner' })) : avatar(p.avatar, p.name || d.user_name || '?', 'lg'),
          h('div', { class: 'grow' }, h('b', null, looking ? 'Looking them up…' : p.name || d.user_name || 'Someone Discord doesn’t know'),
            h('small', null, p.username && p.username !== p.name ? '@' + p.username + ' · ' : '', h('span', { class: 'mono' }, d.user_id))),
          tag,
          h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { d.user_id = ''; d.user_name = ''; person = null; idDraft = ''; draw(); soon(); const b = body.querySelector('#w-member'); if (b) b.focus(); } }, 'Change')));
        if (clash) memberCard.appendChild(h('p', { class: 'welcome-warn' }, icon('alert'), h('span', null, 'There’s already a welcome for them. '), h('a', { href: '#/welcomes/' + clash.id, onclick: (e) => { e.preventDefault(); dr.finish(true); navigate('#/welcomes/' + clash.id); } }, 'Edit that one')));
        if (person && !person.found && !looking) {
          const nameIn = h('input', { class: 'input', id: 'w-name', value: d.user_name, maxlength: '100', placeholder: 'e.g. Lucky', autocomplete: 'off' });
          nameIn.addEventListener('input', () => { d.user_name = nameIn.value; soon(); });
          memberCard.appendChild(field('Name', nameIn, 'Discord doesn’t know this ID, so the panel can’t name them. This is used for {name}. Check the ID if they should exist.', 'w-name'));
        }
      } else {
        const mBtn = h('button', { class: 'picker-btn', type: 'button', id: 'w-member', 'aria-haspopup': 'listbox' }, icon('search'), h('span', { class: 'value placeholder' }, 'Search current members'), icon('chevron'));
        mBtn.addEventListener('click', () => openPicker(mBtn, { title: 'Member', placeholder: 'Search members by name', debounce: 180, load: memberItems,
          onPick: async (it) => {
            d.user_id = it.id; d.user_name = it.member.name;
            person = { found: true, id: it.id, name: it.member.name, username: it.member.username, avatar: it.member.avatar, in_server: true };
            const already = data.items.find((w) => w.user_id === it.id && (!existing || w.id !== existing.id));
            if (already) person.welcome_id = already.id;
            draw(); soon();
          } }));
        const idIn = h('input', { class: 'input mono', id: 'w-id', value: idDraft, placeholder: 'Paste a Discord ID, like 459076776266694670', inputmode: 'numeric', autocomplete: 'off', spellcheck: 'false', maxlength: '24' });
        let idTimer = null;
        idIn.addEventListener('input', () => { idDraft = idIn.value.replace(/[^\d]/g, ''); if (idIn.value !== idDraft) idIn.value = idDraft; clearTimeout(idTimer); lookupError = ''; if (idDraft.length >= 17) idTimer = setTimeout(() => lookup(idDraft), 250); });
        idIn.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); lookup(idIn.value); } });
        append(memberCard, [
          mBtn,
          h('div', { class: 'or-row', 'aria-hidden': 'true' }, 'or, for someone who isn’t in the server'),
          h('div', null, idIn, lookupError ? h('div', { class: 'error-text' }, icon('alert'), lookupError)
            : h('div', { class: 'hint' }, 'In Discord, turn on Developer Mode (Settings › Advanced), then right-click them and Copy User ID.')),
        ]);
      }
      body.appendChild(memberCard);

      // message
      const lines = h('div', { class: 'lines' });
      d.lines.forEach((line, i) => {
        const ta = h('textarea', { class: 'textarea', rows: '2', 'aria-label': 'Version ' + (i + 1), placeholder: i === 0 ? 'e.g. 🎉 {mention} wapas aa gaya! {away} ho gaye the. Welcome home ❤️' : 'Another version', maxlength: '1600' });
        ta.value = line;
        const counter = h('div', { class: 'line-count' + (line.length > 1500 ? ' over' : ''), 'aria-live': 'polite' }, line.length > 1200 ? line.length + ' / 1500' : '');
        const grow = () => { ta.style.height = 'auto'; ta.style.height = Math.min(240, ta.scrollHeight + 2) + 'px'; };
        ta.addEventListener('input', () => { d.lines[i] = ta.value; grow(); counter.textContent = ta.value.length > 1200 ? ta.value.length + ' / 1500' : ''; counter.classList.toggle('over', ta.value.length > 1500); previewIndex = i; drawPreview(); soon(); });
        ta.addEventListener('focus', () => { lastFocused = ta; if (previewIndex !== i) { previewIndex = i; drawPreview(); } });
        requestAnimationFrame(grow);
        lines.appendChild(h('div', { class: 'line-row' }, h('span', { class: 'line-n', 'aria-hidden': 'true' }, i + 1),
          h('div', null, ta, counter),
          h('div', { class: 'line-tools' },
            h('button', { class: 'btn sm ghost icon-only', type: 'button', 'aria-label': 'Remove version ' + (i + 1), disabled: d.lines.length === 1, onclick: () => { d.lines.splice(i, 1); previewIndex = 0; draw(); soon(); } }, icon('trash')))));
      });
      const insert = (text) => {
        const ta = lastFocused && lastFocused.isConnected ? lastFocused : body.querySelector('.line-row textarea');
        if (!ta) return;
        const at = ta.selectionStart === undefined ? ta.value.length : ta.selectionStart;
        ta.value = ta.value.slice(0, at) + text + ta.value.slice(ta.selectionEnd || at);
        ta.dispatchEvent(new Event('input'));
        ta.focus();
        ta.selectionStart = ta.selectionEnd = at + text.length;
      };
      const someone = h('button', { class: 'ph', type: 'button', onmousedown: (e) => e.preventDefault(), 'data-tip': 'Mention another member, and ping them too' }, '@someone');
      someone.addEventListener('click', () => openPicker(someone, { title: 'Mention', placeholder: 'Search members by name', debounce: 180, load: memberItems,
        onPick: (it) => {
          people.set(it.id, it.member);
          if (it.id !== d.user_id && !d.also_ping.includes(it.id) && d.also_ping.length < 10) d.also_ping.push(it.id);
          draw();
          insert('<@' + it.id + '>');
        } }));
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('message'), 'Message', h('span', { class: 'right', style: 'color:var(--faint);font-size:12.5px' }, d.lines.length > 1 ? 'one picked at random' : '')),
        lines,
        h('div', { style: 'display:flex;gap:10px;align-items:center;flex-wrap:wrap' },
          h('button', { class: 'btn sm', type: 'button', disabled: d.lines.length >= 10, onclick: () => { d.lines.push(''); draw(); const all = body.querySelectorAll('.line-row textarea'); all[all.length - 1].focus(); } }, icon('plus'), 'Add a version'),
          h('div', { class: 'placeholders' }, 'Insert:', WELCOME_PLACEHOLDERS.map(([ph, tip]) => h('button', { class: 'ph', type: 'button', onmousedown: (e) => e.preventDefault(), onclick: () => insert(ph), 'data-tip': tip }, ph)), someone)),
        h('p', { class: 'hint', style: 'margin:0' }, 'Up to 10 versions; one goes out at random. **Bold** works as in Discord.')));

      // preview
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('eye'), 'Preview'), preview));

      // pings
      const pingSw = switchEl(d.ping_member, 'Ping them', (on, btn) => { d.ping_member = on; btn.set(on); soon(); }, { small: true });
      const chips = h('div', { class: 'chips' });
      d.also_ping.forEach((id, i) => {
        const p = people.get(id);
        const chip = p ? h('span', { class: 'chip' }, avatar(p.avatar, p.name || id, 'xs'), h('span', { class: 'chip-text' }, p.name || 'Member ' + id),
          h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Don’t ping ' + (p.name || id), onclick: () => { d.also_ping.splice(i, 1); draw(); soon(); } }, icon('x')))
          : memberChip(id, () => { d.also_ping.splice(i, 1); draw(); soon(); });
        chips.appendChild(chip);
      });
      const addBtn = h('button', { class: 'btn sm', type: 'button', disabled: d.also_ping.length >= 10 }, icon('plus'), d.also_ping.length ? 'Add someone' : 'Choose people');
      addBtn.addEventListener('click', () => openPicker(addBtn, { title: 'Also ping', placeholder: 'Search members by name', debounce: 180, load: async (q) => (await memberItems(q)).map((it) => Object.assign(it, { trail: d.also_ping.includes(it.id) ? icon('check', 'check-mark') : null })),
        onPick: (it) => { if (it.id === d.user_id) { toast('They’re the one being welcomed: use “Ping them”.', 'info'); return; } people.set(it.id, it.member); const at = d.also_ping.indexOf(it.id); if (at >= 0) d.also_ping.splice(at, 1); else if (d.also_ping.length < 10) d.also_ping.push(it.id); draw(); soon(); } }));
      chips.appendChild(addBtn);
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('bell'), 'Who gets pinged'),
        h('div', { class: 'toggle-row' }, h('div', { class: 'grow' }, h('b', null, 'Ping them'), h('small', null, d.ping_member ? '{mention} notifies them.' : '{mention} shows their name without a notification.')), pingSw),
        h('div', null, h('label', { class: 'label' }, 'Also ping'), chips, h('div', { class: 'hint' }, 'Mention them in the message with ', h('code', null, '<@ID>'), ' or the @someone button. Only the people here can be pinged: never @everyone or a role.'))));

      // where and how
      const wc = data.welcome_channel ? chan(data.welcome_channel.id) : null;
      const c = d.channel_id ? chan(d.channel_id) : null;
      const chBtn = h('button', { class: 'picker-btn', type: 'button', id: 'w-channel', 'aria-haspopup': 'listbox' }, h('span', { class: 'glyph' }, '#'),
        h('span', { class: 'value' }, d.channel_id ? (c ? c.name : 'unknown-channel') : 'Welcome channel'),
        d.channel_id ? (c && c.category ? h('small', { style: 'color:var(--faint)' }, c.category) : null) : h('small', { style: 'color:var(--faint)' }, wc ? '#' + wc.name : 'not set'), icon('chevron'));
      chBtn.addEventListener('click', () => openPicker(chBtn, { title: 'Channel', placeholder: 'Search channels',
        load: (q) => [{ id: '', label: 'Welcome channel', sub: wc ? '#' + wc.name : 'not set', lead: icon('door'), trail: !d.channel_id ? icon('check', 'check-mark') : null }].filter(() => !q || 'welcome channel'.includes(q.toLowerCase())).concat(channelItems('text', [d.channel_id])(q)),
        onPick: (it) => { d.channel_id = it.id; draw(); } }));
      const replace = h('input', { type: 'checkbox', checked: d.replace_normal });
      replace.addEventListener('change', () => { d.replace_normal = replace.checked; draw(); });
      body.appendChild(h('section', { class: 'form-card' },
        h('h3', null, icon('door'), 'When they join'),
        field('Posts in', chBtn, d.channel_id ? null : 'Wherever the usual welcome goes.', 'w-channel'),
        h('div', null, h('span', { class: 'label' }, 'How often'),
          segmented([['once', 'Once', 'check'], ['every', 'Every time they join', 'repeat']], d.mode, 'How often', (v) => { d.mode = v; draw(); }),
          h('div', { class: 'hint' }, d.mode === 'once' ? 'Switches itself off after it has gone out. Switch it back on to welcome them again.' : 'Stays on: posts each time they join.')),
        h('label', { class: 'check' }, replace, h('span', null, 'Replace the normal welcome', h('small', null, d.replace_normal ? 'Posts instead of Loduchand’s usual welcome line.' : 'Posts right under the usual welcome line.'), h('small', null, 'The house sorting card follows either way.')))));

      // note
      const note = h('textarea', { class: 'textarea', id: 'w-note', rows: '2', maxlength: '500', placeholder: 'e.g. Asked for by Kohli on 12 Sep' });
      note.value = d.note;
      note.addEventListener('input', () => { d.note = note.value; });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('edit'), 'Note', h('span', { class: 'right', style: 'color:var(--faint);font-size:12.5px' }, 'only admins see this')), note));

      drawPreview();
      body.scrollTop = scroll;
    };
    draw();
    refresh();
    requestAnimationFrame(() => { const first = body.querySelector(existing ? '.line-row textarea' : '#w-member'); if (first) first.focus({ preventScroll: true }); });
  }

  // --- the arena: starting a battle royale by hand ----------------------------------------

  /** The last length and tag the admin picked, so the dialog remembers them. */
  const ARENA = { data: null, minutes: null, ping: 'houses', busy: false };

  /** How the toast says who was tagged, kept grammatical. */
  const TAGGED = { houses: 'the four houses were tagged', warriors: 'the Warrior role was tagged', everyone: 'everyone was tagged', none: 'nobody was tagged' };

  const arenaMinutes = (n, d) => Math.min(Math.max(Math.round(n) || d.default_minutes, d.min_minutes), d.max_minutes);

  /** The dialog: where it posts, how long the lobby stays open, who gets tagged. */
  async function askBattle(d) {
    let ping = d.pings.some((p) => p.key === ARENA.ping) ? ARENA.ping : d.pings[0].key;
    const num = h('input', { class: 'input', type: 'number', inputmode: 'numeric', id: 'arena-minutes',
      min: String(d.min_minutes), max: String(d.max_minutes), step: '1', value: String(arenaMinutes(ARENA.minutes || d.default_minutes, d)) });
    const about = h('p', { class: 'hint arena-about' }, (d.pings.find((p) => p.key === ping) || {}).about);
    const tags = segmented(d.pings.map((p) => [p.key, p.emoji + ' ' + p.label]), ping, 'Who gets tagged', (v) => {
      ping = v;
      about.textContent = (d.pings.find((p) => p.key === v) || {}).about || '';
    });
    const field = (label, control, hint) => h('div', { class: 'arena-field' },
      h('label', { for: control.id || null }, label), control, hint || null);
    const ok = await confirmDialog({ title: 'Start a battle royale now?', icon: 'zap', confirm: 'Open the lobby',
      body: h('div', { class: 'arena-ask' },
        h('p', null, 'The lobby goes up in ', d.channel ? channelRef(d.channel.id) : h('b', null, 'the arena'),
          ' this instant and anyone can join while it is open. When it closes the bracket runs itself, and the champion takes the role and the points.'),
        field('Lobby open for', h('div', { class: 'arena-minutes' }, num, h('span', null, 'minutes')),
          h('p', { class: 'hint' }, 'Between ' + d.min_minutes + ' and ' + d.max_minutes + ' · ' + d.default_minutes + ' if you leave it')),
        field('Tag', tags, about)) });
    if (!ok) return null;
    ARENA.minutes = arenaMinutes(parseInt(num.value, 10) || d.default_minutes, d);
    ARENA.ping = ping;
    return { minutes: ARENA.minutes, ping: ARENA.ping };
  }

  /** The extra part of the Arena page, above its settings: start one now. */
  function renderArena(page) {
    const holder = h('div', { class: 'arena-start' });
    page.appendChild(holder);
    const load = () => api('GET', '/arena').then((d) => { ARENA.data = d; }).catch(() => null);
    const draw = () => {
      clear(holder);
      const d = ARENA.data;
      const btn = h('button', { class: 'btn primary', type: 'button' }, icon('zap'),
        h('span', { class: 'hide-sm' }, 'Start a battle royale now'), h('span', { class: 'show-sm' }, 'Start one now'));
      btn.disabled = !d || !d.channel || ARENA.busy;
      let body;
      if (!d) {
        body = h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading the arena…');
      } else if (!d.channel) {
        body = h('div', { class: 'banner inline', role: 'alert' }, icon('alert'),
          h('p', null, h('b', null, 'There is nowhere to fight yet. '),
            h('span', null, 'Pick a fight channel in the settings below, or make a channel called #fight-fight-fight.')));
      } else {
        body = h('div', { class: 'arena-now' },
          h('p', null, 'A lobby opens in ', channelRef(d.channel.id), ' straight away, tagging ',
            h('b', null, (d.pings.find((p) => p.key === ARENA.ping) || d.pings[0]).label.toLowerCase()),
            ', and stays open ' + plural(arenaMinutes(ARENA.minutes || d.default_minutes, d), 'minute') + ' for people to join.'),
          h('p', { class: 'hint' }, 'Exactly the lobby /battle and the daily battle open — this one just doesn’t wait for the clock.'));
      }
      btn.addEventListener('click', async () => {
        const ask = await askBattle(ARENA.data);
        if (!ask) return;
        ARENA.busy = true;
        btn.disabled = true;
        try {
          const res = await api('POST', '/arena/battle', ask);
          const where = res.channel && res.channel.name ? '#' + res.channel.name : 'the arena';
          toast('Lobby open for ' + plural(res.minutes, 'minute') + ' in ' + where + ' — ' + (TAGGED[res.ping] || res.ping_label + ' was tagged'));
          refreshAudit();
        } catch (e) { toast(e.message, 'error'); }
        ARENA.busy = false;
        await load();
        if (holder.isConnected) draw();
      });
      holder.appendChild(card('arena-now', 'Battle royale', 'Open a lobby without waiting for the daily one', body, { pad: true, actions: btn }));
    };
    draw();
    load().then(() => { if (holder.isConnected) draw(); });
  }

  // --- chocolate frogs -------------------------------------------------------------------

  const FROG = { data: null, rewards: null, sales: null, trades: null, tradeFilter: 'all', tradeMember: null, allDrops: false, ownerMode: 'member', member: null, wizard: '', owners: null, ownersBusy: false };

  async function loadFrogs() {
    FROG.data = await api('GET', '/frogs');
    return FROG.data;
  }

  function rarityOf(key) {
    return ((FROG.data && FROG.data.rarities) || []).find((r) => r.key === key) || { key, name: key, emoji: '🐸', colour: '#8b93ff', points: 0, difficulty: 'easy', chance: 0 };
  }
  function rarityChip(key) {
    const r = rarityOf(key);
    return h('span', { class: 'badge rarity', style: '--rarity:' + r.colour }, h('span', { class: 'swatch', style: 'background:' + r.colour }), r.emoji + ' ' + r.name);
  }
  function frogArt(url, cls) {
    return url ? h('img', { class: 'frog-art' + (cls ? ' ' + cls : ''), src: url, alt: '', loading: 'lazy', decoding: 'async' })
      : h('span', { class: 'frog-art placeholder' + (cls ? ' ' + cls : ''), 'aria-hidden': 'true' }, '🐸');
  }
  const serialLabel = (n) => 'No. ' + String(n).padStart(4, '0');
  const pointsWords = (n) => n + (n === 1 ? ' point' : ' points');
  function secsWords(n) { if (n < 60) return n + ' s'; const m = Math.floor(n / 60), s = n % 60; return m + ' min' + (s ? ' ' + s + ' s' : ''); }

  const TRADE_STATUS = { open: ['Open', 'live'], done: ['Done', 'on'], declined: ['Declined', 'paused'], cancelled: ['Cancelled', 'paused'], expired: ['Expired', 'paused'], failed: ['Failed', 'failed'] };
  const personName = (p) => (p && p.name) || (p && p.id ? 'Member ' + p.id : 'Someone');
  /** "Sun 13 Sep", built by hand: browsers disagree on "Sep" and "Sept". */
  const fullDay = (ymd) => { const [y, m, d] = String(ymd).split('-').map(Number); if (!y) return ymd; const wd = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'][new Date(Date.UTC(y, m - 1, d)).getUTCDay()]; return wd + ' ' + d + ' ' + ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'][m - 1]; };
  function cardChip(c) {
    const r = rarityOf(c.rarity);
    return h('span', { class: 'frog-card-chip', style: '--rarity:' + r.colour, title: c.name ? c.name + ' #' + c.edition + ' · ' + serialLabel(c.serial) : serialLabel(c.serial) },
      h('span', null, (c.rarity ? r.emoji + ' ' : '') + (c.name ? c.name + ' #' + c.edition : 'Card')), h('small', { class: 'mono' }, serialLabel(c.serial)));
  }
  function originWords(c) {
    if (c.origin === 'caught') return 'Caught by ' + personName(c.original_owner);
    if (c.origin === 'royale_champion') return 'Won by ' + personName(c.original_owner) + ' as royale champion';
    if (c.origin === 'royale_runner_up') return 'Won by ' + personName(c.original_owner) + ' as royale runner-up';
    if (String(c.origin).startsWith('daily_top:')) return 'Earned by ' + personName(c.original_owner) + ' as top of the day';
    return 'Earned by ' + personName(c.original_owner);
  }

  function frogEarnedCard(r) {
    const body = h('div', { class: 'frog-earned' });
    if (!r) { body.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading…')); return card('frog-earned', 'Earned cards', null, body, { pad: true }); }
    const days = [];
    r.daily.forEach((a) => { let g = days.find((x) => x.day === a.day); if (!g) { g = { day: a.day, rows: [] }; days.push(g); } g.rows.push(a); });
    const daily = h('section', { class: 'frog-earned-col' }, h('h3', null, '☀️ Top of the day', r.daily_enabled ? null : h('span', { class: 'badge paused' }, 'Off')));
    if (!days.length) daily.appendChild(h('p', { class: 'hint' }, 'Each morning the day before’s top scorer in every game wins a card. None yet.'));
    days.slice(0, 7).forEach((g) => {
      daily.appendChild(h('div', { class: 'frog-day' }, h('b', null, fullDay(g.day)), h('small', null, plural(g.rows.length, 'card'))));
      daily.appendChild(h('ul', { class: 'frog-award-list' }, g.rows.map((a) => h('li', null,
        h('span', { class: 'frog-award-what' }, a.activity_label), avatar(a.member.avatar, personName(a.member), 'xs'),
        h('span', { class: 'frog-award-who' }, h('b', null, personName(a.member)), h('small', null, pointsWords(a.total) + ' that day')), cardChip(a.card)))));
    });
    const royale = h('section', { class: 'frog-earned-col' }, h('h3', null, '⚔️ Battle royales', r.royale_enabled ? null : h('span', { class: 'badge paused' }, 'Off')));
    if (!r.royale.length) royale.appendChild(h('p', { class: 'hint' }, 'The champion and runner-up of a big enough royale win a card each. None yet.'));
    else royale.appendChild(h('ul', { class: 'frog-award-list' }, r.royale.slice(0, 20).map((a) => h('li', null,
      h('span', { class: 'frog-award-what' }, a.role === 'champion' ? '👑 Champion' : '🥈 Runner-up'), avatar(a.member.avatar, personName(a.member), 'xs'),
      h('span', { class: 'frog-award-who' }, h('b', null, personName(a.member)), h('small', null, dayMonth(a.ts))), cardChip(a.card)))));
    const sold = h('section', { class: 'frog-earned-col' }, h('h3', null, '🏆 Full sets sold'));
    const sales = FROG.sales ? FROG.sales.items : [];
    if (!sales.length) sold.appendChild(h('p', { class: 'hint' }, 'Members hand in one copy of every card with /sellset for ' + pointsWords(FROG.sales ? FROG.sales.price : 35) + '. None sold yet.'));
    else sold.appendChild(h('ul', { class: 'frog-sale-list' }, sales.slice(0, 20).map((x) => h('li', null,
      h('div', { class: 'frog-sale-head' }, avatar(x.member.avatar, personName(x.member), 'xs'), h('b', null, personName(x.member)), h('span', { class: 'badge on' }, '+' + x.points),
        h('span', { class: 'grow' }), h('small', { title: fmtFull.format(new Date(x.ts * 1000)) + ' IST' }, dayMonth(x.ts))),
      h('div', { class: 'frog-chips' }, x.cards.map(cardChip))))));
    append(body, [daily, royale, sold]);
    return card('frog-earned', 'Earned cards & sales', 'Cards won by playing (no points) · full sets handed in for points', body, { pad: true });
  }

  function frogTradesCard(redraw) {
    const t = FROG.trades;
    const body = h('div', { class: 'frog-trades' });
    const controls = h('div', { class: 'frog-owner-controls' });
    controls.appendChild(segmented([['all', 'All'], ['open', 'Open'], ['done', 'Done'], ['closed', 'Closed']], FROG.tradeFilter, 'Trade status', (v) => { FROG.tradeFilter = v; redraw(); }));
    const who = FROG.tradeMember;
    const btn = h('button', { class: 'picker-btn', type: 'button', 'aria-haspopup': 'listbox' }, icon('search'), h('span', { class: 'value' + (who ? '' : ' placeholder') }, who ? who.name : 'Any member'), icon('chevron'));
    btn.addEventListener('click', () => openPicker(btn, { title: 'Member', placeholder: 'Search members by name', debounce: 180,
      load: async (q) => [{ id: '', label: 'Any member', lead: icon('users') }].filter(() => !q).concat(await memberItems(q)),
      onPick: async (it) => { FROG.tradeMember = it.id ? it.member : null; await loadTrades(); redraw(); } }));
    controls.appendChild(btn);
    body.appendChild(controls);
    if (!t) { body.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading trades…')); }
    else {
      const shown = t.items.filter((x) => FROG.tradeFilter === 'all' || (FROG.tradeFilter === 'closed' ? !['open', 'done'].includes(x.status) : x.status === FROG.tradeFilter));
      if (!shown.length) body.appendChild(h('p', { class: 'hint' }, t.items.length ? 'No trades match.' : 'No trades yet. Members start one with /trade.'));
      else body.appendChild(h('ul', { class: 'frog-trade-list' }, shown.map((x) => {
        const [label, cls] = TRADE_STATUS[x.status] || [x.status, ''];
        const link = x.guild_id && x.message_id ? h('a', { href: 'https://discord.com/channels/' + x.guild_id + '/' + x.channel.id + '/' + x.message_id, target: '_blank', rel: 'noopener' }, icon('external'), 'Open in Discord') : null;
        const cancel = x.status === 'open' ? h('button', { class: 'btn sm danger', type: 'button', onclick: async () => {
          const ok = await confirmDialog({ title: 'Cancel this offer?', icon: 'alert', danger: true, confirm: 'Cancel offer', body: personName(x.from) + '’s offer to ' + personName(x.to) + ' closes and its message in Discord says it was cancelled. No cards move.' });
          if (!ok) return;
          try { await api('POST', '/frogs/trades/' + x.id + '/cancel'); toast('Offer cancelled'); refreshAudit(); await loadTrades(); redraw(); } catch (e) { toast(e.message, 'error'); }
        } }, icon('x'), 'Cancel offer') : null;
        const side = (title, list) => h('div', { class: 'frog-trade-side' }, h('small', null, title), list.length ? h('div', { class: 'frog-chips' }, list.map(cardChip)) : h('span', { class: 'hint' }, 'nothing'));
        return h('li', { class: 'frog-trade is-' + x.status },
          h('div', { class: 'frog-trade-head' }, h('span', { class: 'badge ' + cls }, x.status === 'open' ? h('span', { class: 'dot' }) : null, label),
            avatar(x.from.avatar, personName(x.from), 'xs'), h('b', null, personName(x.from)), h('span', { class: 'frog-arrow' }, x.status === 'done' ? '⇄' : '→'), avatar(x.to.avatar, personName(x.to), 'xs'), h('b', null, personName(x.to)),
            h('span', { class: 'grow' }), h('small', { title: fmtFull.format(new Date(x.created_ts * 1000)) + ' IST' }, x.status === 'open' ? 'expires ' + fromNow(x.expires_ts) : ago(x.closed_ts || x.created_ts))),
          h('div', { class: 'frog-trade-sides' }, side(personName(x.from) + ' gives', x.give), side(personName(x.from) + ' asks for', x.ask)),
          (link || cancel) ? h('div', { class: 'frog-trade-foot' }, x.channel && x.channel.name ? h('small', null, '#' + x.channel.name) : null, link, h('span', { class: 'grow' }), cancel) : null);
      })));
    }
    const c = t && t.counts;
    return card('frog-trades', 'Trades', c ? c.open + ' open · ' + c.done + ' done · ' + (c.declined + c.cancelled + c.expired + c.failed) + ' closed' : null, body, { pad: true });
  }

  async function loadTrades() {
    try { FROG.trades = await api('GET', '/frogs/trades' + (FROG.tradeMember ? '?member=' + encodeURIComponent(FROG.tradeMember.id) : '')); } catch (_) { FROG.trades = FROG.trades || { items: [], counts: null }; }
  }

  /** The extra parts of the Chocolate Frogs page, above its settings. */
  function renderFrogs(page) {
    const holder = h('div', { class: 'frog-page' });
    page.appendChild(holder);
    const draw = () => {
      clear(holder);
      const d = FROG.data;
      if (!d) { holder.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading the frogs…')); return; }
      drawFrogTiles(holder, d);
      holder.appendChild(frogWizardsCard(d, draw));
      holder.appendChild(frogDropsCard(d, draw));
      holder.appendChild(frogOwnersCard(d));
      holder.appendChild(frogEarnedCard(FROG.rewards));
      holder.appendChild(frogTradesCard(draw));
      holder.appendChild(frogBankCard(d));
    };
    draw();
    Promise.all([api('GET', '/frogs/rewards').then((r) => { FROG.rewards = r; }).catch(() => null), api('GET', '/frogs/sales').then((r) => { FROG.sales = r; }).catch(() => null), loadTrades()]).then(() => { if (holder.isConnected && FROG.data) draw(); });
    loadFrogs().then(() => { if (holder.isConnected) draw(); }).catch((e) => { if (holder.isConnected) { clear(holder); holder.appendChild(h('div', { class: 'banner inline', role: 'alert' }, icon('alert'), h('p', null, e.message))); } });
  }

  function drawFrogTiles(holder, d) {
    const t = d.totals;
    const chans = d.channels.map((c) => '#' + (c.name || c.id) + (d.channels.length > 1 ? ' ×' + c.weight : '')).join(', ');
    const onTile = tile('Scheduled drops', 'zap', d.enabled ? 'On' : 'Off',
      d.enabled ? h('span', null, h('span', { class: 'ok' }, '● '), 'Frogs drop on their own') : h('a', { href: '#/s/frogs?k=VIZIER_FROGS' }, 'Switch on in the settings below'));
    holder.appendChild(h('div', { class: 'tiles frog-tiles' },
      onTile,
      tile('Drop channels', 'hash', d.channels.length ? String(d.channels.length) : 'None',
        d.channels.length ? h('span', { title: chans }, (d.channels_from === 'snitch' ? 'The Snitch’s: ' : '') + chans) : 'Set drop channels below'),
      tile('Frogs dropped', 'activity', numberFmt.format(t.dropped), t.caught + ' caught · ' + t.escaped + ' escaped' + (t.open ? ' · ' + t.open + ' open' : '')),
      tile('Cards owned', 'tag', numberFmt.format(t.cards), plural(t.collectors, 'collector') + ' · ' + plural(t.full_sets, 'full set'))));
  }

  function frogWizardsCard(d, redraw) {
    const legend = h('div', { class: 'frog-legend' }, d.rarities.map((r) =>
      h('span', { class: 'frog-legend-item', style: '--rarity:' + r.colour, 'data-tip': r.wizards ? 'About ' + r.chance + '% of drops · ' + r.difficulty + ' riddles' : 'No card of this rarity is switched on, so it never drops' },
        h('span', { class: 'swatch' }), h('b', null, r.emoji + ' ' + r.name), h('span', null, pointsWords(r.points) + ' · ' + r.chance + '%'))));
    const grid = h('div', { class: 'frog-grid' });
    d.wizards.forEach((w) => {
      const r = rarityOf(w.rarity);
      const sw = switchEl(w.enabled, (w.enabled ? 'Switch off ' : 'Switch on ') + w.name, async (on, btn) => {
        btn.disabled = true;
        try {
          const saved = await api('PUT', '/frogs/wizards/' + w.id, { name: w.name, rarity: w.rarity, image: w.image, enabled: on });
          Object.assign(w, saved);
          toast(w.name + (on ? ' is back in the game' : ' won’t drop any more'));
          refreshAudit();
          redraw();
        } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
      }, { small: true, noText: true });
      grid.appendChild(h('article', { class: 'frog-wizard' + (w.enabled ? '' : ' is-off'), style: '--rarity:' + r.colour, 'data-wizard': w.id },
        h('button', { class: 'frog-wizard-open', type: 'button', 'aria-label': 'Edit ' + w.name, onclick: () => openWizardEditor(w, redraw) },
          frogArt(w.image_url), h('span', { class: 'frog-wizard-rarity' }, r.emoji)),
        h('div', { class: 'frog-wizard-meta' },
          h('b', { title: w.name }, w.name),
          h('small', null, r.name + ' · ' + pointsWords(r.points)),
          h('small', { class: 'frog-copies' }, w.copies ? '×' + w.copies + ' owned' : 'none owned yet')),
        h('div', { class: 'frog-wizard-foot' }, sw, h('button', { class: 'btn sm ghost', type: 'button', onclick: () => openWizardEditor(w, redraw) }, icon('edit'), 'Edit'))));
    });
    if (d.wizards.length < d.max_wizards) grid.appendChild(h('button', { class: 'new-card frog-new', type: 'button', onclick: () => openWizardEditor(null, redraw) }, icon('plus'), 'Add a card', h('small', null, 'Another one to collect')));
    const on = d.wizards.filter((w) => w.enabled).length;
    return card('frog-wizards', 'Cards', on + ' of ' + d.wizards.length + ' in the game', [legend, grid], {
      pad: true, actions: h('button', { class: 'btn sm', type: 'button', onclick: () => openWizardEditor(null, redraw) }, icon('plus'), h('span', { class: 'hide-sm' }, 'Add a card')),
    });
  }

  function frogStatus(x) {
    if (x.status === 'caught') {
      const who = x.winner || {};
      return h('div', { class: 'frog-status' }, avatar(who.avatar, who.name || '?', 'xs'),
        h('span', null, h('b', null, who.name || 'Member ' + (who.id || '')), h('small', null, 'caught it in ' + secsWords(x.solved_secs || 0))));
    }
    if (x.status === 'open') {
      const left = Math.max(0, x.closes_at - Math.round(Date.now() / 1000));
      return h('span', { class: 'badge live' }, h('span', { class: 'dot' }), left ? 'Open · ' + Math.ceil(left / 60) + ' min left' : 'Closing');
    }
    return h('span', { class: 'badge paused' }, 'Escaped');
  }

  function frogDropsCard(d, redraw) {
    const dropBtn = h('button', { class: 'btn sm primary', type: 'button' }, icon('send'), h('span', { class: 'hide-sm' }, 'Drop a frog now'), h('span', { class: 'show-sm' }, 'Drop'));
    dropBtn.addEventListener('click', () => openPicker(dropBtn, { title: 'Channel', placeholder: 'Drop a frog in…',
      load: (q) => channelItems('text')(q).filter((c) => !/safe-corner/i.test(c.label)),
      onPick: async (it) => {
        const ok = await confirmDialog({ title: 'Drop a frog in #' + it.label + ' now?', icon: 'send', confirm: 'Drop the frog',
          body: h('div', null, h('p', null, 'A real Chocolate Frog hops in straight away, with a random card and riddle. Whoever answers first keeps the card and scores the points.'),
            h('p', null, 'It works even when scheduled drops are off, and doesn’t use up one of today’s drops.')) });
        if (!ok) return;
        try {
          const res = await api('POST', '/frogs/drop', { channel_id: it.id });
          toast(rarityOf(res.rarity).emoji + ' ' + res.wizard + ' hopped into #' + (res.channel.name || it.label));
          refreshAudit();
          await loadFrogs();
          redraw();
        } catch (e) { toast(e.message, 'error'); }
      } }));
    let body;
    if (!d.drops.length) {
      body = h('div', { class: 'empty' }, h('span', { class: 'frog-empty', 'aria-hidden': 'true' }, '🐸'), h('h3', null, 'No frogs yet'), h('p', null, 'Drops show here with who caught them. Try one with “Drop a frog now”.'));
    } else {
      body = h('div', { class: 'table-wrap' }, h('table', { class: 'log frog-drops' },
        h('thead', null, h('tr', null, ['When', 'Where', 'Card', 'What happened', 'Number', ''].map((t) => h('th', null, t)))),
        h('tbody', null, d.drops.slice(0, FROG.allDrops ? d.drops.length : 12).map((x) => {
          const r = rarityOf(x.rarity);
          const riddle = x.riddle;
          return h('tr', null,
            h('td', { class: 'when', title: fmtFull.format(new Date(x.ts * 1000)) + ' IST' }, ago(x.ts), h('small', null, fmtDateTime.format(new Date(x.ts * 1000)))),
            h('td', { class: 'where' }, channelRef(x.channel.id), x.by ? h('small', { class: 'frog-test' }, 'test by ' + (x.by.name || 'an admin')) : null),
            h('td', { class: 'wizard' }, h('b', null, r.emoji + ' ' + x.wizard), h('small', null, r.name + ' · ' + pointsWords(x.points))),
            h('td', { class: 'change-cell' }, frogStatus(x)),
            h('td', { class: 'serial mono' }, x.serial ? '#' + x.edition + ' · ' + serialLabel(x.serial) : '—'),
            h('td', { class: 'row-actions' }, riddle ? h('button', { class: 'btn sm ghost' + (riddle.retired ? ' is-retired' : ''), type: 'button', onclick: () => riddleDialog(riddle, redraw) },
              icon(riddle.retired ? 'pause' : 'message'), riddle.retired ? 'Retired' : 'Riddle') : null));
        }))));
      if (d.drops.length > 12) {
        body = [body, h('div', { class: 'frog-more' }, h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { FROG.allDrops = !FROG.allDrops; redraw(); } },
          icon(FROG.allDrops ? 'up' : 'down'), FROG.allDrops ? 'Show fewer' : 'Show all ' + d.drops.length))];
      }
    }
    return card('frog-drops', 'Recent drops', d.drops.length ? 'The last ' + d.drops.length + ' · frogs stay open ' + d.open_minutes + ' min' : null, body, { actions: dropBtn });
  }

  async function riddleDialog(riddle, redraw) {
    const retire = !riddle.retired;
    const ok = await confirmDialog({ title: 'Riddle ' + riddle.id, icon: 'message', wide: true, danger: retire,
      confirm: retire ? 'Retire this riddle' : 'Put it back in play',
      body: h('div', { class: 'frog-riddle' },
        h('blockquote', null, riddle.text),
        h('p', null, h('b', null, 'Answer: '), riddle.answer, riddle.answers.length > 1 ? h('span', { class: 'frog-alts' }, ' · also accepted: ' + riddle.answers.slice(1).join(', ')) : null),
        h('p', { class: 'hint' }, riddle.difficulty + ' · ' + (riddle.topic || 'no topic') + (riddle.retired ? ' · retired: it is never asked' : '')),
        retire ? h('p', null, 'Retire it if the answer is wrong, unclear or unfair. It stops being asked; frogs already in chat keep it.') : null) });
    if (!ok) return;
    try {
      await api('POST', '/frogs/riddles/' + encodeURIComponent(riddle.id) + '/retire', { retired: retire });
      toast(retire ? 'Riddle ' + riddle.id + ' won’t be asked again' : 'Riddle ' + riddle.id + ' is back in play');
      refreshAudit();
      await loadFrogs();
      redraw();
    } catch (e) { toast(e.message, 'error'); }
  }

  function frogOwnersCard(d) {
    const body = h('div', { class: 'frog-owners' });
    const results = h('div', { class: 'frog-owner-results', 'aria-live': 'polite' });
    const fetchOwners = async (query) => {
      FROG.ownersBusy = true; drawResults();
      try { FROG.owners = await api('GET', '/frogs/owners?' + query); } catch (e) { FROG.owners = null; toast(e.message, 'error'); }
      FROG.ownersBusy = false; if (results.isConnected) drawResults();
    };
    const drawResults = () => {
      clear(results);
      if (FROG.ownersBusy) { results.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Looking…')); return; }
      const o = FROG.owners;
      if (!o) { results.appendChild(h('p', { class: 'hint' }, FROG.ownerMode === 'member' ? 'Pick a member to see their cards.' : 'Pick a card to see who owns its copies.')); return; }
      if (o.member && FROG.ownerMode === 'member') {
        const m = o.member;
        results.appendChild(h('div', { class: 'person-row' }, avatar(m.avatar, m.name || '?', 'lg'),
          h('div', { class: 'grow' }, h('b', null, m.name || 'Member ' + m.id), h('small', null, plural(o.cards.length, 'card') + ' · ' + o.collected + ' of ' + o.of + ' collected · ' + pointsWords(o.points) + ' from frogs'))));
        if (!o.cards.length) results.appendChild(h('p', { class: 'hint' }, 'No cards yet.'));
        else results.appendChild(h('ul', { class: 'frog-cards' }, o.cards.map((c) => {
          const moves = (c.transfers || []).map((t) => personName(t.from) + ' → ' + personName(t.to) + ' · ' + dayMonth(t.ts));
          const mark = c.traded_in ? '🔁' : c.origin !== 'caught' ? '🏅' : '';
          return h('li', { style: '--rarity:' + rarityOf(c.rarity).colour, title: [originWords(c)].concat(moves).join('\n') },
            h('span', null, rarityOf(c.rarity).emoji + ' ' + c.wizard + ' #' + c.edition), h('span', { class: 'mono' }, serialLabel(c.serial)), h('small', null, (mark ? mark + ' ' : '') + dayMonth(c.ts)),
            moves.length ? h('small', { class: 'frog-history' }, originWords(c) + ' · traded ' + moves.length + '×: ' + moves.join(', ')) : null);
        })));
      } else if (o.wizard && FROG.ownerMode === 'wizard') {
        const w = o.wizard;
        results.appendChild(h('div', { class: 'person-row' }, frogArt(w.image_url, 'sm'),
          h('div', { class: 'grow' }, h('b', null, w.name), h('small', null, plural(w.copies, 'card') + ' · ' + plural(o.owners.length, 'owner')))));
        if (!o.owners.length) results.appendChild(h('p', { class: 'hint' }, 'Nobody has caught ' + w.name + ' yet.'));
        else results.appendChild(h('ul', { class: 'frog-holders' }, o.owners.map((x) => h('li', null,
          avatar(x.member.avatar, x.member.name || '?', 'xs'), h('b', null, x.member.name || 'Member ' + x.member.id),
          h('span', { class: 'frog-serials' }, x.copies.map((c) => h('span', { class: 'chip mono', title: 'Copy #' + c.edition + ', card ' + serialLabel(c.serial) }, '#' + c.edition + ' · ' + serialLabel(c.serial))))))));
      }
    };
    const controls = h('div', { class: 'frog-owner-controls' });
    const drawControls = () => {
      clear(controls);
      controls.appendChild(segmented([['member', 'By member', 'user'], ['wizard', 'By card', 'tag']], FROG.ownerMode, 'Find cards', (v) => { FROG.ownerMode = v; FROG.owners = null; drawControls(); drawResults(); }));
      if (FROG.ownerMode === 'member') {
        const btn = h('button', { class: 'picker-btn', type: 'button', 'aria-haspopup': 'listbox' }, icon('search'), h('span', { class: 'value' + (FROG.member ? '' : ' placeholder') }, FROG.member ? FROG.member.name : 'Search members'), icon('chevron'));
        btn.addEventListener('click', () => openPicker(btn, { title: 'Member', placeholder: 'Search members by name', debounce: 180, load: memberItems,
          onPick: (it) => { FROG.member = it.member; drawControls(); fetchOwners('member=' + encodeURIComponent(it.id)); } }));
        controls.appendChild(btn);
      } else {
        const sel = h('select', { class: 'select', 'aria-label': 'Card' }, h('option', { value: '' }, 'Pick a card'),
          d.wizards.map((w) => h('option', { value: w.id, selected: String(FROG.wizard) === String(w.id) }, rarityOf(w.rarity).emoji + ' ' + w.name + (w.copies ? ' (' + w.copies + ')' : ''))));
        sel.addEventListener('change', () => { FROG.wizard = sel.value; if (sel.value) fetchOwners('wizard=' + encodeURIComponent(sel.value)); else { FROG.owners = null; drawResults(); } });
        controls.appendChild(sel);
      }
    };
    drawControls();
    drawResults();
    append(body, [controls, results]);
    return card('frog-owners', 'Card owners', 'Who owns which numbered cards', body, { pad: true });
  }

  function frogBankCard(d) {
    const total = d.bank.reduce((n, b) => n + b.playable, 0);
    const forRarity = { easy: 'Common', medium: 'Uncommon', hard: 'Legendary' };
    const rows = h('div', { class: 'frog-bank' }, d.bank.map((b) => {
      const pct = b.playable ? Math.round((b.unused / b.playable) * 100) : 0;
      return h('div', { class: 'frog-bank-row' },
        h('div', { class: 'frog-bank-label' }, h('b', null, b.difficulty[0].toUpperCase() + b.difficulty.slice(1)), h('small', null, forRarity[b.difficulty])),
        h('div', { class: 'frog-bar', role: 'img', 'aria-label': b.unused + ' of ' + b.playable + ' not asked yet' }, h('span', { style: 'width:' + pct + '%' })),
        h('div', { class: 'frog-bank-num' }, h('b', null, numberFmt.format(b.unused)), ' of ' + numberFmt.format(b.playable) + ' left', b.retired ? h('small', null, b.retired + ' retired') : null));
    }));
    const note = h('p', { class: 'hint', style: 'margin:0' }, 'Each riddle is asked once before any repeats; when a level runs out it starts over. The bank is read from riddlebank/ai on every start. Retire a bad riddle from its drop above.');
    return card('frog-bank', 'Riddle bank', numberFmt.format(total) + ' riddles in play', [rows, note], { pad: true });
  }

  function openWizardEditor(w, redraw) {
    const d = w ? { name: w.name, rarity: w.rarity, image: w.image, enabled: w.enabled } : { name: '', rarity: 'common', image: '', enabled: true };
    const original = JSON.stringify(d);
    const dirty = () => JSON.stringify(d) !== original;
    const data = FROG.data;
    const dr = openDrawer({
      title: w ? w.name : 'New card',
      sub: w ? (w.copies ? plural(w.copies, 'card') + ' owned so far' : 'Nobody owns this card yet') : 'A new card for the Chocolate Frog game',
      saveLabel: w ? 'Save changes' : 'Add card', isDirty: dirty,
      onSave: async (btn) => {
        if (!d.name.trim()) { toast('Give the card a name.', 'error'); return; }
        btn.disabled = true;
        try {
          const saved = w ? await api('PUT', '/frogs/wizards/' + w.id, d) : await api('POST', '/frogs/wizards', d);
          dr.finish(true);
          await loadFrogs().catch(() => null);
          redraw();
          toast(w ? 'Saved ' + saved.name : saved.name + ' joined the game');
          refreshAudit();
        } catch (e) { toast(e.message, 'error'); btn.disabled = false; }
      },
    });
    const body = dr.body;
    body.classList.add('frog-editor');
    const artUrl = () => (d.image ? '/api/media/' + encodeURIComponent(d.image) : (w && w.image_source === 'file' && !w.image ? w.image_url : null));
    const field = (label, control, hint, id) => h('div', null, h('label', { class: 'label', for: id || null }, label), control, hint ? h('div', { class: 'hint' }, hint) : null);

    const draw = () => {
      const scroll = body.scrollTop;
      clear(body);
      const r = rarityOf(d.rarity);
      const name = d.name.trim() || 'Card name';

      // preview
      const url = artUrl();
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('eye'), 'How it drops'),
        h('div', { class: 'preview' }, h('div', { class: 'msg' }, h('span', { class: 'brand-mark' }, h('span', null, 'L')),
          h('div', { style: 'min-width:0' }, h('div', { class: 'msg-head' }, h('b', null, 'Loduchand'), h('span', { class: 'msg-bot' }, 'BOT'), h('span', { class: 'msg-time' }, 'Today')),
            h('div', { class: 'embed frog-embed', style: '--embed:' + r.colour },
              h('div', { class: 'frog-embed-main' },
                h('div', { class: 'embed-title' }, '🐸 A Chocolate Frog hopped in!'),
                h('div', { class: 'embed-desc' }, h('strong', null, name), ' · ' + r.emoji + ' ' + r.name + ' · ', h('strong', null, pointsWords(r.points))),
                h('div', { class: 'frog-subtext' }, 'First correct answer keeps the card · 3 tries · ' + data.open_minutes + ' min')),
              url ? h('img', { class: 'frog-thumb', src: url, alt: '' }) : null),
            h('span', { class: 'frog-discord-btn' }, '🐸 Catch it'))))));

      // details
      const nameIn = h('input', { class: 'input', id: 'fw-name', value: d.name, maxlength: String(data.max_name), placeholder: 'e.g. The Moonkeeper', autocomplete: 'off' });
      nameIn.addEventListener('input', () => { d.name = nameIn.value; const t = body.querySelector('.frog-embed strong'); if (t) t.textContent = d.name.trim() || 'Card name'; });
      const enabled = switchEl(d.enabled, 'In the game', (on, btn) => { d.enabled = on; btn.set(on); });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('tag'), 'Card', h('span', { class: 'right' }, enabled)),
        field('Name', nameIn, 'Up to ' + data.max_name + ' characters. Shown on the card and in the pop-up title.', 'fw-name'),
        h('div', null, h('span', { class: 'label' }, 'Rarity'),
          segmented(data.rarities.map((x) => [x.key, x.emoji + ' ' + x.name]), d.rarity, 'Rarity', (v) => { d.rarity = v; draw(); }),
          h('div', { class: 'hint' }, pointsWords(r.points) + ' · ' + r.difficulty + ' riddles · about ' + r.chance + '% of drops are ' + r.name + '. Points and odds are in the settings below.')),
        h('p', { class: 'hint', style: 'margin:0' }, d.enabled ? 'In the game: this card can turn up on a drop.' : 'Out of the game: this card stops dropping. Copies already caught stay with their owners.')));

      // picture
      const pics = h('div', { class: 'frog-pic-row' }, url ? h('img', { class: 'frog-pic', src: url, alt: 'The card picture' }) : h('span', { class: 'frog-pic placeholder', 'aria-hidden': 'true' }, '🐸'),
        h('div', { class: 'frog-pic-tools' },
          h('button', { class: 'btn sm', type: 'button', onclick: () => { const sel = d.image ? [d.image] : []; openLibrary(sel, () => { const next = sel.length ? sel[sel.length - 1] : ''; if (next !== d.image) { d.image = next; draw(); } }); } }, icon('image'), 'Choose from library'),
          d.image ? h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { d.image = ''; draw(); } }, icon('x'), 'Remove picture') : null,
          h('div', { class: 'hint' }, d.image ? 'From the picture library.' : w && w.image_source === 'file' && !w.image ? 'Using frogcards/' + w.slug + ' from the server.' : 'No picture: the card drops without one. You can also put frogcards/' + (w ? w.slug : 'name') + '.png on the server.')));
      const zone = dropZone('Upload a picture', 'PNG, JPG or WebP, up to 8 MB. Square pictures look best; Discord gets a small copy.', async (files) => {
        zone.classList.add('busy');
        const saved = await uploadPictures([files[0]]);
        zone.classList.remove('busy');
        if (saved[0]) { d.image = saved[0].id; draw(); }
      });
      body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('image'), 'Picture'), pics, zone));

      // owners
      if (w) {
        const list = h('div', { class: 'frog-owner-results' }, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading owners…'));
        body.appendChild(h('section', { class: 'form-card' }, h('h3', null, icon('users'), 'Owners', h('span', { class: 'right', style: 'color:var(--faint);font-size:12.5px' }, plural(w.copies, 'card'))), list));
        api('GET', '/frogs/owners?wizard=' + w.id).then((o) => {
          clear(list);
          if (!o.owners.length) { list.appendChild(h('p', { class: 'hint', style: 'margin:0' }, 'Nobody has caught ' + w.name + ' yet.')); return; }
          list.appendChild(h('ul', { class: 'frog-holders' }, o.owners.map((x) => h('li', null, avatar(x.member.avatar, x.member.name || '?', 'xs'), h('b', null, x.member.name || 'Member ' + x.member.id),
            h('span', { class: 'frog-serials' }, x.copies.map((c) => h('span', { class: 'chip mono', title: 'Copy #' + c.edition + ', card ' + serialLabel(c.serial) }, '#' + c.edition + ' · ' + serialLabel(c.serial))))))));
        }).catch((e) => { clear(list); list.appendChild(h('p', { class: 'error-text' }, e.message)); });
      }
      body.scrollTop = scroll;
    };
    draw();
    requestAnimationFrame(() => { const n = body.querySelector('#fw-name'); if (n && !w) n.focus({ preventScroll: true }); });
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
        opts.onDelete ? h('button', { class: 'btn danger', type: 'button', onclick: opts.onDelete, 'aria-label': 'Delete' }, icon('trash'), h('span', { class: 'hide-sm' }, 'Delete')) : null,
        opts.footExtra || null,
        h('span', { class: 'grow' }),
        h('button', { class: 'btn ghost' + (opts.footExtra ? ' hide-sm' : ''), type: 'button', onclick: () => attempt() }, 'Cancel'),
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
    { id: 'games', label: 'Koto, anagram, cats, Wordle, frogs', icon: '🔤', sources: ['koto', 'anagram', 'guess', 'movie', 'cat', 'wordle', 'frog'] },
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
      const unitWord = c.unit === 'msgs' ? 'messages' : 'minutes';
      const capTxt = c.cap ? pts + '/' + c.cap : String(pts);
      if (c.reached) { cls += ' reached'; pct = 1; text = [icon('check'), 'max ' + c.cap]; }
      else text = (pts ? capTxt + ' · ' : '') + c.count + '/' + c.target + ' ' + c.unit;
      if (!c.count && !pts) cls += ' zero';
      title = c.label + ': ' + capTxt + ' points today, ' + c.count + ' ' + unitWord + (c.reached ? ' (daily limit reached)' : ', next point at ' + c.target) + (c.with_company ? ' · voice counts only with someone else in the room' : '');
    } else if (c.kind === 'capped') {
      pct = c.cap ? Math.min(1, pts / c.cap) : 0;
      if (c.reached) { cls += ' reached'; text = [icon('check'), 'max ' + c.cap]; }
      else text = c.cap ? pts + '/' + c.cap : pts + ' · no limit';
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
      h('span', null, h('span', { class: 'act' }, h('span', { class: 'act-icon' }, '💬'), h('span', { class: 'act-text' }, '14/20 msgs'), h('span', { class: 'act-bar' }, h('i', { style: 'width:70%' }))), ' on the way to the next chat or voice point'),
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
        h('span', null, (st.day === 'today' ? 'Today, ' : 'Yesterday, ') + fmtDay(st.data.day) + ' · chat points at 20/60/150 messages (as set), a voice point per ' + st.data.voice_bar_min + ' minutes with company'),
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
    page.appendChild(activeSection());
    page.appendChild(h('div', { class: 'section-title' }, h('h2', null, 'Find a member')));
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

    const reviewHolder = h('div', null);
    page.insertBefore(reviewHolder, page.querySelector('.active-wrap'));
    const notesCard = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading notes…'));
    page.appendChild(h('div', { class: 'section-title' }, h('h2', null, 'Members with notes'), h('span', null, 'Private to mods. Members never see these.')));
    page.appendChild(notesCard);
    api('GET', '/members/notes').then((all) => {
      const list = all.filter((n) => !n.awaits_review);
      drawReview(reviewHolder, all.filter((n) => n.awaits_review));
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
  }

  /** Auto-filled notes still switched off: select and switch on in bulk. */
  function drawReview(holder, items) {
    clear(holder);
    if (!items.length) return;
    const chosen = new Set();
    const btn = h('button', { class: 'btn sm primary', type: 'button', disabled: true }, icon('check'), 'Switch on for selected');
    const all = h('input', { type: 'checkbox', 'aria-label': 'Select all notes to review' });
    const boxes = [];
    const sync = () => {
      btn.disabled = !chosen.size;
      btn.lastChild.textContent = chosen.size ? 'Switch on for selected (' + chosen.size + ')' : 'Switch on for selected';
      all.checked = chosen.size === items.length;
      all.indeterminate = chosen.size > 0 && chosen.size < items.length;
    };
    all.addEventListener('change', () => { boxes.forEach(([b, id]) => { b.checked = all.checked; if (all.checked) chosen.add(id); else chosen.delete(id); }); sync(); });
    const list = h('ul', { class: 'review-list' }, items.map((n) => {
      const box = h('input', { type: 'checkbox', 'aria-label': 'Select ' + n.name });
      boxes.push([box, n.user_id]);
      box.addEventListener('change', () => { if (box.checked) chosen.add(n.user_id); else chosen.delete(n.user_id); sync(); });
      return h('li', { class: 'review-row' }, h('label', { class: 'active-check' }, box),
        h('a', { class: 'review-main', href: '#/members/' + n.user_id }, avatar(n.avatar, n.name, 'lg'),
          h('div', { class: 'grow' }, h('div', { class: 'title-row' }, h('b', null, n.name), h('span', { class: 'badge tone-' + n.tone }, (TONE_ICONS[n.tone] || '') + ' ' + toneLabel(n.tone))),
            h('p', null, n.notes || 'Tone only, no notes.')),
          h('small', null, 'Filled ' + ago(n.filled_ts || n.updated_ts)), icon('right')));
    }));
    btn.addEventListener('click', async () => {
      const ids = [...chosen];
      const ok = await confirmDialog({ title: 'Switch on ' + plural(ids.length, 'note') + '?', icon: 'check', confirm: 'Switch on',
        body: 'From now on the bot uses these notes and tones when it answers these members. Make sure you’ve read them: they were written by the analysis, not a mod.' });
      if (!ok) return;
      try {
        const r = await api('POST', '/members/notes/enable', { user_ids: ids });
        toast('Switched on ' + plural(r.enabled.length, 'note'));
        refreshAudit(); refreshStatus(); rerender();
      } catch (e) { toast(e.message, 'error'); }
    });
    append(holder, [
      h('div', { class: 'section-title' }, h('h2', null, 'Notes to review'), h('span', null, plural(items.length, 'note') + ' filled in by the analysis, switched off until you read them')),
      h('div', { class: 'card review-card' },
        h('div', { class: 'active-toolbar' }, h('label', { class: 'check', style: 'align-items:center' }, all, h('span', null, 'Select all')), h('span', { class: 'grow' }), btn),
        list),
    ]);
  }

  function toneLabel(t) { return ({ normal: 'Normal', gentle: 'Gentle', light_roast: 'Light roast', roast: 'Roast', respectful: 'Respectful', brief: 'Brief' })[t] || t; }

  const profileState = { id: null, tab: 'overview', data: null };

  async function renderProfile(page, id, tab) {
    document.title = 'Member · Loduchand';
    profileState.tab = ['overview', 'seen', 'memories', 'analysis', 'connections', 'about'].includes(tab) ? tab : 'overview';
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
          p.joins ? h('span', null, icon('repeat'), plural(p.joins.joins, 'join') + ', ' + plural(p.joins.leaves, 'leave')) : null,
          h('a', { class: 'open-link', href: '#/messages?member=' + encodeURIComponent(p.id) }, icon('message'), 'Read their messages'),
          h('a', { class: 'open-link', href: '#/deleted?member=' + encodeURIComponent(p.id) }, icon('trash'), 'Deleted messages')),
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
      if (key === 'connections') profileConnections(panel, p);
      else if (key === 'analysis') profileAnalysis(panel, p, show);
      else if (key === 'about') profileAbout(panel, p);
      else if (key === 'seen') profileSeen(panel, p);
      else if (key === 'memories') profileMemories(panel, p);
      else profileOverview(panel, p);
    };
    const tabs = h('nav', { class: 'page-tabs', 'aria-label': 'Profile sections' },
      [['overview', 'Overview', 'overview'], ['connections', 'Connections', 'spark'], ['analysis', 'Bot’s analysis', 'flask'], ['about', '/about notes', 'user'], ['seen', 'What the bot sees', 'message'], ['memories', 'What it remembers', 'bot']].map(([key, label, ic]) =>
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
    const labels = { chat: '💬 Chat', voice: '🎙️ Voice', quiz: '🧠 Quiz', koto: '🔤 Koto', anagram: '🔡 Anagram', guess: '🎨 Guess the Word', movie: '🎬 Guess the Movie', cat: '🐱 Cat Bot', wordle: '🟩 Wordle', arena: '⚔️ Arena', royale: '👑 Battle Royale', snitch: '🪽 Snitch', golden_snitch: '🥇 Golden Snitch', frog: '🐸 Chocolate Frog', weekly: '📝 Weekly posts', mod: '🛡️ Mods' };
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

  // What /about says about a member, when "what they're like" was built and
  // what it cost, who looked them up, and rebuild / clear. Opening this is
  // itself a lookup and goes in the activity log.
  async function profileAbout(panel, p) {
    panel.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading their notes…'));
    let got, all;
    try { [got, all] = await Promise.all([api('GET', '/notes/' + p.id), api('GET', '/notes').catch(() => null)]); }
    catch (e) { clear(panel).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); return; }
    if (!panel.isConnected) return;
    clear(panel);
    const again = () => { if (panel.isConnected) { clear(panel); profileAbout(panel, p); } };
    const tokens = (n) => numberFmt.format(n.input_tokens) + ' in + ' + numberFmt.format(n.output_tokens) + ' out';
    if (!got.enabled) {
      panel.appendChild(h('section', { class: 'card' }, h('div', { class: 'empty' }, icon('user'), h('h3', null, 'Member notes are off'),
        h('p', null, 'Switch them on under Settings → About members (VIZIER_NOTES). Until then /about says they are off and nothing is built.'))));
    }
    if (got.preview) {
      panel.appendChild(card('pf-about-preview', 'What /about shows', 'Exactly the private answer they (or a mod) would get · opening this is logged', h('pre', { class: 'ai-preview' }, got.preview), { pad: true }));
    }
    const n = got.note;
    const rebuild = h('button', { class: 'btn primary sm', type: 'button', disabled: got.opted_out || !got.enabled }, icon('restart'), n ? 'Rebuild now' : 'Build now');
    rebuild.addEventListener('click', async () => {
      rebuild.disabled = true; rebuild.lastChild.textContent = 'Asking the model…';
      try { const r = await api('POST', '/notes/' + p.id + '/rebuild'); toast('Rebuilt · ' + tokens(r.note) + ' tokens'); refreshAudit(); again(); }
      catch (e) { toast(e.message, 'error'); rebuild.disabled = false; rebuild.lastChild.textContent = n ? 'Rebuild now' : 'Build now'; }
    });
    const clearBtn = n ? h('button', { class: 'btn ghost sm', type: 'button' }, icon('trash'), 'Clear') : null;
    if (clearBtn) clearBtn.addEventListener('click', async () => {
      const yes = await confirmDialog({ title: 'Clear ' + p.name + '’s notes?', icon: 'trash', danger: true, confirm: 'Clear notes',
        body: '“What they’re like” is deleted now. They aren’t opted out, so a later run can build it again once they have ' + got.new_messages + ' new messages.' });
      if (!yes) return;
      try { await api('DELETE', '/notes/' + p.id); toast('Cleared'); refreshAudit(); again(); } catch (e) { toast(e.message, 'error'); }
    });
    let body;
    if (got.opted_out) body = h('p', { class: 'hint' }, icon('shield'), ' They opted out with /forgetme: nothing is kept and no build will include them until they opt back in.');
    else if (n) body = h('div', null,
      h('ul', { class: 'about-bullets' }, n.bullets.map((b) => h('li', null, b))),
      h('p', { class: 'hint' }, 'Built ' + when(n.built_ts) + ' (' + ago(n.built_ts) + ')' + (n.built_by ? ' by a mod' : ' by the scheduled run') + ' · ' + n.model +
        ' · read ' + plural(n.messages_used, 'message') + ' · ' + tokens(n) + ' tokens' + (n.rejected ? ' · ' + plural(n.rejected, 'bullet') + ' thrown out by the filter' : '')));
    else body = h('p', { class: 'hint' }, got.attempt ? 'Not built: ' + got.attempt.outcome + ' (' + ago(got.attempt.ts) + ').' : 'Not built yet. The first build covers members with at least ' + got.min_messages + ' messages.');
    panel.appendChild(card('pf-about-note', 'What they’re like', 'Written by the notes model, stored, and shown with no model call', body, { pad: true, actions: h('div', { class: 'row-actions' }, rebuild, clearBtn) }));
    const looks = got.lookups.length
      ? h('ul', { class: 'about-lookups' }, got.lookups.map((l) => h('li', null, (l.asker_name || l.asker) + ' · ' + l.via + ' · ' + when(l.ts))))
      : h('p', { class: 'hint' }, 'Nobody has looked them up.');
    panel.appendChild(card('pf-about-lookups', 'Lookups by mods', 'Newest first · also in the activity log', looks, { pad: true }));
    if (all) {
      const runs = all.runs.length
        ? h('ul', { class: 'about-lookups' }, all.runs.map((r) => h('li', null, when(r.ts) + ' · ' + r.kind + ' · ' + plural(r.asked, 'model call') + ', ' + r.built + ' built, ' + r.skipped + ' too little, ' + r.failed + ' failed · ' + tokens(r) + ' tokens' + (r.waiting ? ' · ' + r.waiting + ' waiting' : ''))))
        : h('p', { class: 'hint' }, 'No runs yet.');
      panel.appendChild(card('pf-about-runs', 'All builds', all.built + ' members have notes · ' + all.opted_out + ' opted out · spent ' + tokens(all.spent) + ' tokens · a full first build now: ' +
        plural(all.estimate.members, 'member') + ', about ' + numberFmt.format(all.estimate.input_tokens) + ' + ' + numberFmt.format(all.estimate.output_tokens) + ' tokens', runs, { pad: true }));
    }
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
          // Ask for the text as if switched on, so it can be shown dimmed when it isn't.
          const r = await api('POST', '/members/' + p.id + '/note/preview', Object.assign({}, d, { use_in_replies: true }));
          if (mine !== pseq) return;
          preview.textContent = r.text || 'Nothing yet — add notes or pick a tone, and this is what the bot will get.';
          preview.classList.toggle('empty-preview', !r.text);
          preview.classList.toggle('off-preview', !!r.text && !d.use_in_replies);
          if (!d.use_in_replies) {
            previewNote.textContent = 'Switched off: the bot gets nothing for them right now.';
            previewNote.className = 'hint off-note';
          } else {
            previewNote.textContent = r.switched_on ? 'Added before any message of theirs the bot answers, and messages that mention them.' : 'Member notes are switched off for the whole bot, so the AI gets none right now.';
            previewNote.className = r.switched_on ? 'hint' : 'error-text';
          }
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
        if (autoBanner && autoBanner.isConnected) { autoBanner.remove(); refreshStatus(); }
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
    const autoBanner = saved && saved.source === 'analysis' && !saved.reviewed
      ? h('div', { class: 'auto-banner', role: 'status' }, icon('flask'),
        h('div', { class: 'grow' }, h('p', null, 'Filled in from the bot’s analysis on ' + fmtDate((saved.filled_ts || saved.updated_ts) * 1000) + '. Read and edit it, then switch on “Use when replying”.'),
          h('div', { class: 'auto-banner-actions' },
            h('button', { class: 'btn sm primary', type: 'button', onclick: async () => {
              if (dirty()) { toast('Save or undo your changes first.', 'error'); return; }
              try {
                await api('POST', '/members/notes/enable', { user_ids: [p.id] });
                toast('The bot now uses the notes for ' + p.name);
                refreshAudit(); refreshStatus();
                p.note = Object.assign({}, p.note, { use_in_replies: true, reviewed: true });
                el.replaceWith(notesEditor(p));
              } catch (e) { toast(e.message, 'error'); }
            } }, icon('check'), 'Looks good — use it'),
            h('a', { class: 'open-link', href: '#/members/' + p.id + '/analysis' }, 'See the analysis', icon('right')))))
      : null;
    append(el, [
      h('div', { class: 'card-head' }, h('div', { class: 'grow' }, h('h2', null, 'Mods’ notes'), h('div', { class: 'sub' }, icon('shield'), ' Private to mods. Never shown to members.'))),
      autoBanner,
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

  // --- member analyses ----------------------------------------------------------------------

  const activeState = { by: 'overall', tier: 'all', hours: 720, limit: 20, selected: new Set(), data: null, job: null, poll: null };
  const TIER_LABEL = { very: 'Very active', fair: 'Fairly active', less: 'Less active' };
  const STATUS_LABEL = { draft: 'Draft', reviewed: 'Reviewed' };

  function analysisChip(profile, id) {
    if (!profile) return h('span', { class: 'badge off' }, 'No analysis');
    const cls = profile.status === 'reviewed' ? 'badge on' : 'badge src-panel';
    return h('a', { class: cls + ' analysis-chip', href: '#/members/' + id + '/analysis' }, profile.status === 'reviewed' ? icon('check') : icon('flask'), STATUS_LABEL[profile.status] || profile.status);
  }

  async function startAnalysis(body) {
    try {
      const r = await api('POST', '/profiles/analyse', body);
      activeState.job = r.job;
      return r.job;
    } catch (e) {
      toast(e.message, 'error');
      return null;
    }
  }

  /** Polls the job while `holder` is on the page, redrawing it; `onDone` runs once when it ends. */
  function watchJob(holder, onDone) {
    clearInterval(activeState.poll);
    let wasRunning = activeState.job && activeState.job.running;
    const draw = () => { clear(holder); const el = jobPanel(activeState.job); if (el) holder.appendChild(el); };
    const tick = async () => {
      if (!holder.isConnected) { clearInterval(activeState.poll); return; }
      try {
        const r = await api('GET', '/profiles/job');
        activeState.job = r.job;
      } catch (_) { return; }
      draw();
      const running = activeState.job && activeState.job.running;
      if (wasRunning && !running) { clearInterval(activeState.poll); if (onDone) onDone(activeState.job); }
      if (!running) clearInterval(activeState.poll);
      wasRunning = running;
    };
    draw();
    tick();
    activeState.poll = setInterval(tick, 2000);
  }

  function jobPanel(job) {
    if (!job) return null;
    const finishedLongAgo = job.finished_ts && Date.now() / 1000 - job.finished_ts > 15 * 60;
    if (finishedLongAgo) return null;
    const settled = job.done + job.skipped + job.failed + job.items.filter((i) => i.status === 'cancelled').length;
    const pct = job.total ? Math.round((settled / job.total) * 100) : 0;
    const running = job.items.find((i) => i.status === 'running');
    const title = job.running
      ? (job.cancelled ? 'Stopping after the current member…' : 'Analysing ' + (running ? running.name : 'members') + '…')
      : (job.cancelled ? 'Analysis cancelled' : 'Analysis finished');
    const summary = [job.done + ' analysed', job.skipped ? job.skipped + ' recent, not re-analysed' : null, job.failed ? job.failed + ' failed' : null,
      job.fill_notes ? job.notes_filled + ' notes filled' : null, job.fill_notes && job.notes_kept ? job.notes_kept + ' kept (mod-written)' : null].filter(Boolean).join(' · ');
    return h('div', { class: 'job' + (job.running ? ' running' : ''), role: 'status' },
      h('div', { class: 'job-head' }, job.running ? h('span', { class: 'spinner' }) : icon(job.failed ? 'alert' : 'check'),
        h('div', { class: 'grow' }, h('b', null, title), h('small', null, settled + ' of ' + job.total + ' · ' + summary)),
        job.running && !job.cancelled ? h('button', { class: 'btn sm', type: 'button', onclick: async () => {
          try { const r = await api('DELETE', '/profiles/job'); activeState.job = r.job; toast('Stopping after the current member', 'info'); } catch (e) { toast(e.message, 'error'); }
        } }, icon('x'), 'Cancel') : null),
      h('div', { class: 'job-bar', 'aria-hidden': 'true' }, h('i', { style: 'width:' + pct + '%' })),
      h('ul', { class: 'job-items' }, job.items.map((i) => h('li', { class: 'job-item ' + i.status, title: i.detail || '' },
        h('span', { class: 'job-dot', 'aria-hidden': 'true' }), h('a', { href: '#/members/' + i.user_id + '/analysis' }, i.name),
        h('small', null, [({ queued: 'waiting', running: 'analysing', done: 'analysed', skipped: 'recent', failed: 'failed', cancelled: 'cancelled' })[i.status] || i.status,
          i.note ? ' · ' + (({ filled: 'note filled', refilled: 'note refilled', kept: 'note kept', empty: 'nothing to fill' })[i.note] || 'note failed') : ''].join(''))))));
  }

  function activeSection() {
    const st = activeState;
    const wrap = h('section', { class: 'active-wrap', 'aria-label': 'Most active members' });
    const jobHolder = h('div', null);
    const list = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Ranking members…'));
    const selectedBtn = h('button', { class: 'btn sm', type: 'button', disabled: true }, icon('flask'), 'Analyse selected');
    const topBtn = h('button', { class: 'btn sm primary', type: 'button', disabled: true }, icon('zap'), 'Analyse all active');
    const tierBar = h('div', { class: 'tier-bar', role: 'radiogroup', 'aria-label': 'Activity tier' });
    const PERIODS = [[24, '24 hours'], [48, '48 hours'], [168, '7 days'], [720, '30 days']];
    const periodBar = h('div', { class: 'tier-bar period-bar', role: 'radiogroup', 'aria-label': 'Time period' });
    const periodLabel = () => (PERIODS.find(([n]) => n === st.hours) || PERIODS[3])[1];
    const drawPeriods = () => {
      clear(periodBar);
      periodBar.appendChild(h('span', { class: 'period-label' }, icon('clock'), 'Period'));
      PERIODS.forEach(([n, label]) => periodBar.appendChild(h('button', { type: 'button', role: 'radio', class: 'tier-chip', 'aria-checked': st.hours === n ? 'true' : 'false',
        onclick: () => { if (st.hours === n) return; st.hours = n; st.limit = 20; drawPeriods(); load(); } }, label)));
    };
    drawPeriods();
    const updateSelected = () => {
      selectedBtn.disabled = !st.selected.size;
      selectedBtn.lastChild.textContent = st.selected.size ? 'Analyse selected (' + st.selected.size + ')' : 'Analyse selected';
    };
    const confirmRun = async (count, body) => {
      const ok = await confirmDialog({ title: 'Analyse ' + plural(count, 'member') + '?', icon: 'flask', confirm: 'Start',
        body: h('div', { class: 'restart-body' },
          h('p', null, 'The bot’s model reads each member’s last 30 days of messages it has stored (never #safe-corner or DMs) and their numbers, one member at a time. It takes a few seconds each.'),
          h('p', null, 'Members analysed in the last day are skipped. The results are drafts for mods; the bot doesn’t use them until you add them to notes.')) });
      if (!ok) return;
      const job = await startAnalysis(body);
      if (job) { st.selected.clear(); updateSelected(); watchJob(jobHolder, () => load()); drawList(); }
    };
    selectedBtn.addEventListener('click', () => confirmRun(st.selected.size, { user_ids: [...st.selected] }));
    topBtn.addEventListener('click', async () => {
      const counts = (st.data && st.data.tiers) || { very: 0, fair: 0 };
      const per = (st.data && st.data.seconds_per_member) || 20;
      const opts = { fair: true, force: false, fill: true };
      const estimate = h('p', { class: 'estimate' });
      const drawEstimate = () => {
        const n = Math.min(300, counts.very + (opts.fair ? counts.fair : 0));
        const minutes = Math.max(1, Math.round((n * per) / 60));
        clear(estimate);
        append(estimate, [icon('clock'), h('b', null, plural(n, 'member')), ' · about ' + plural(minutes, 'minute') + (opts.force ? '' : ' at most (recent ones are skipped)'),
          counts.very + counts.fair > 300 && opts.fair ? h('small', null, ' Only the 300 most active are taken.') : null]);
      };
      const check = (key, label, help) => {
        const box = h('input', { type: 'checkbox', checked: opts[key] });
        box.addEventListener('change', () => { opts[key] = box.checked; drawEstimate(); });
        return h('label', { class: 'check' }, box, h('span', null, label, h('small', null, help)));
      };
      drawEstimate();
      const ok = await confirmDialog({ title: 'Analyse all active members?', icon: 'flask', confirm: 'Start', wide: true,
        body: h('div', { class: 'review' },
          h('p', null, 'The bot’s model reads each member’s last 30 days of stored messages (never #safe-corner or DMs) and their numbers, one member at a time, with a short pause in between.'),
          check('fair', 'Include fairly active members', plural(counts.fair, 'fairly active member') + ' on top of ' + plural(counts.very, 'very active one') + '.'),
          check('force', 'Re-analyse ones done in the last day', 'Off skips anyone analysed in the last 24 hours.'),
          check('fill', 'Fill in their Mods’ notes', 'Writes a note for members who have none (or only an untouched auto-filled one), switched off until you review it. Notes a mod wrote are never changed.'),
          estimate) });
      if (!ok) return;
      const job = await startAnalysis({ tier: opts.fair ? 'fair_and_very' : 'very', force: opts.force, fill_notes: opts.fill });
      if (job) { watchJob(jobHolder, () => { load(); refreshStatus(); }); drawList(); }
    });

    const load = async () => {
      try { st.data = await api('GET', '/profiles/active?by=' + st.by + '&limit=100&hours=' + st.hours + (st.tier === 'all' ? '' : '&tier=' + st.tier)); }
      catch (e) { clear(list).appendChild(h('div', { class: 'empty' }, h('p', null, e.message))); return; }
      drawTiers();
      drawList();
    };
    const drawTiers = () => {
      const c = st.data.tiers;
      const all = c.very + c.fair + c.less;
      const active = Math.min(300, c.very + c.fair);
      topBtn.disabled = !active;
      topBtn.lastChild.textContent = 'Analyse all active (' + active + ')';
      clear(tierBar);
      [['all', 'All', all], ['very', 'Very active', c.very], ['fair', 'Fairly active', c.fair], ['less', 'Less active', c.less]].forEach(([key, label, n]) => {
        tierBar.appendChild(h('button', { type: 'button', role: 'radio', class: 'tier-chip tier-' + key, 'aria-checked': st.tier === key ? 'true' : 'false',
          onclick: () => { if (st.tier === key) return; st.tier = key; st.limit = 20; load(); } }, key === 'all' ? null : h('i', { 'aria-hidden': 'true' }), label, h('b', null, n)));
      });
      const t = st.data.thresholds;
      tierBar.appendChild(h('a', { class: 'tier-rule', href: '#/s/members', 'data-tip': (st.hours < 720 ? 'Bars scaled to ' + periodLabel() + '. ' : '') + 'Very active: ' + t.very_messages + ' messages, ' + t.very_voice_minutes + ' voice minutes or ' + t.very_points + ' game points. Fairly active: ' + t.fair_messages + ', ' + t.fair_voice_minutes + ' or ' + t.fair_points + '. Any one is enough.' }, icon('sliders'), 'Tier bars'));
    };
    const drawList = () => {
      clear(list);
      if (!st.data) return;
      const rows = st.data.rows.slice(0, st.limit);
      if (!rows.length) { list.appendChild(h('div', { class: 'empty' }, icon('users'), h('h3', null, 'No activity yet'), h('p', null, 'Messages, voice and game points from the last ' + periodLabel() + ' show up here.'))); return; }
      const max = (k) => Math.max(1, ...st.data.rows.map((r) => r[k]));
      const [mm, mv, mp] = [max('messages'), max('voice_min'), max('points')];
      const table = h('ol', { class: 'active-list' });
      rows.forEach((r) => {
        const box = h('input', { type: 'checkbox', checked: st.selected.has(r.id), 'aria-label': 'Select ' + (r.name || r.id) });
        box.addEventListener('change', () => { if (box.checked) st.selected.add(r.id); else st.selected.delete(r.id); updateSelected(); });
        const metric = (value, of, text, slot, sort) => h('span', { class: 'metric' + (st.by === sort ? ' sorted' : '') },
          h('b', null, text), h('span', { class: 'metric-bar', 'aria-hidden': 'true' }, h('i', { style: 'width:' + Math.max(value ? 3 : 0, Math.round((value / of) * 100)) + '%;background:var(--s' + slot + ')' })));
        table.appendChild(h('li', { class: 'active-row' },
          h('label', { class: 'active-check' }, box),
          h('span', { class: 'sr-rank' }, r.rank),
          h('div', { class: 'sr-who' }, avatar(r.avatar, r.name || '?', 'lg'),
            h('div', { style: 'min-width:0' }, h('a', { class: 'sr-name', href: '#/members/' + r.id }, r.name || 'Former member'),
              h('span', { class: 'active-tags' }, r.house ? h('span', { class: 'house-chip', style: '--house:' + r.house.colour }, r.house.crest + ' ' + r.house.name) : null,
                h('span', { class: 'badge tier-badge tier-' + r.tier }, TIER_LABEL[r.tier]),
                r.muggle ? h('span', { class: 'badge paused' }, 'Muggle') : null))),
          metric(r.messages, mm, numberFmt.format(r.messages) + ' msgs', 1, 'chat'),
          metric(r.voice_min, mv, duration(r.voice_min * 60), 2, 'voice'),
          metric(r.points, mp, numberFmt.format(r.points) + ' pts', 3, 'games'),
          h('span', { class: 'active-status' }, analysisChip(r.profile, r.id))));
      });
      list.appendChild(h('div', { class: 'active-head', 'aria-hidden': 'true' }, h('span', null), h('span', null), h('span', null, 'Member'),
        h('span', null, h('i', { style: 'background:var(--s1)' }), 'Chat'), h('span', null, h('i', { style: 'background:var(--s2)' }), 'Voice'),
        h('span', null, h('i', { style: 'background:var(--s3)' }), 'Game points'), h('span', null, 'Analysis')));
      list.appendChild(table);
      if (st.data.rows.length > st.limit) list.appendChild(h('div', { class: 'more-row' }, h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { st.limit = 100; drawList(); } }, 'Show ' + (st.data.total > 100 ? 'the top 100 of ' + st.data.total : 'all ' + st.data.total))));
    };

    append(wrap, [
      h('div', { class: 'section-title' }, h('h2', null, 'Most active'), h('span', null, 'Chat, voice and games')),
      h('div', { class: 'card active-card' },
        h('div', { class: 'active-toolbar' },
          segmented([['overall', 'Overall'], ['chat', 'Chat'], ['voice', 'Voice'], ['games', 'Games']], st.by, 'Rank by', (v) => { st.by = v; st.data = null; clear(list).appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Ranking members…')); load(); }),
          h('span', { class: 'grow' }), selectedBtn, topBtn),
        periodBar,
        tierBar,
        h('p', { class: 'active-explain' }, icon('info'), h('span', null, h('b', null, 'Overall'), ' is the average of each member’s share of all messages, all voice time and all game points on the server in the chosen period, so being big in one counts as much as being steady in all three. Tiers need any one of their bars (scaled down for shorter periods). Bots are left out; mods’ and weekly awards don’t count as game points.')),
        jobHolder, list),
    ]);
    updateSelected();
    load();
    api('GET', '/profiles/job').then((r) => { activeState.job = r.job; watchJob(jobHolder, () => load()); }).catch(() => {});
    return wrap;
  }

  // The analysis tab on a member's profile.
  async function profileAnalysis(panel, p, show) {
    panel.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading the analysis…'));
    let got;
    try { got = await api('GET', '/profiles/' + p.id); } catch (e) { clear(panel).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); return; }
    if (!panel.isConnected) return;
    clear(panel);
    const jobHolder = h('div', null);
    const reanalyse = async (force) => {
      const job = await startAnalysis({ user_ids: [p.id], force: true });
      if (!job) return;
      watchJob(jobHolder, (j) => {
        const item = j && j.items.find((i) => i.user_id === p.id);
        if (item && item.status === 'failed') toast('The analysis failed: ' + (item.detail || 'unknown error'), 'error');
        if (panel.isConnected) { clear(panel); profileAnalysis(panel, p, show); }
      });
    };
    const inputBtn = h('button', { class: 'btn sm ghost', type: 'button', onclick: () => showPrompt(p) }, icon('eye'), 'Show the exact input');
    const seenLink = h('a', { class: 'open-link', href: '#/members/' + p.id + '/seen', onclick: (e) => { e.preventDefault(); show('seen'); } }, 'What the bot sees', icon('right'));
    const privacy = h('p', { class: 'analysis-privacy' }, icon('shield'), h('span', null, 'Drafts are for mods only. The bot doesn’t use them until you add them to notes.'));

    if (!got.profile) {
      panel.appendChild(h('section', { class: 'card' },
        h('div', { class: 'empty analysis-empty' }, icon('flask'), h('h3', null, 'No analysis yet'),
          h('p', null, 'The bot’s model can read what ' + p.name + ' wrote in the last 30 days (never #safe-corner or DMs) and their numbers, and draft a summary for mods to edit.'),
          h('div', { class: 'analysis-empty-actions' }, h('button', { class: 'btn primary', type: 'button', onclick: () => reanalyse(false) }, icon('flask'), 'Analyse now'), inputBtn),
          privacy),
        jobHolder));
      api('GET', '/profiles/job').then((r) => { activeState.job = r.job; if (r.job && r.job.running) watchJob(jobHolder, () => { if (panel.isConnected) { clear(panel); profileAnalysis(panel, p, show); } }); }).catch(() => {});
      return;
    }

    const prof = got.profile;
    const draft = {};
    Object.keys(prof.fields).forEach((k) => { draft[k] = JSON.parse(JSON.stringify(prof.fields[k].value)); });
    const base = JSON.stringify(draft);
    const dirtyFields = () => Object.keys(draft).filter((k) => JSON.stringify(draft[k]) !== JSON.stringify(prof.fields[k].value));
    S.guards = (S.guards || []).filter((g) => g.kind !== 'analysis');
    S.guards.push({ kind: 'analysis', count: () => (panel.isConnected ? dirtyFields().length : 0) });

    const saveBtn = h('button', { class: 'btn primary', type: 'button', disabled: true }, 'Save edits');
    const dirtyNote = h('span', { class: 'notes-status' });
    const refreshState = () => {
      const n = dirtyFields().length;
      saveBtn.disabled = !n;
      clear(dirtyNote);
      if (n) dirtyNote.appendChild(h('span', { class: 'badge unsaved' }, h('span', { class: 'dot' }), plural(n, 'unsaved edit')));
    };

    const statusChip = prof.status === 'reviewed' ? h('span', { class: 'badge on' }, icon('check'), 'Reviewed') : h('span', { class: 'badge src-panel' }, icon('flask'), 'Draft');
    const byName = memberName(prof.generated_by);
    const meta = h('p', { class: 'analysis-meta' },
      'Generated ' + fmtFull.format(new Date(prof.generated_ts * 1000)) + ' IST by ', byName,
      ' · last ' + prof.window_days + ' days · ' + plural(prof.messages_analysed, 'message') + ' (' + numberFmt.format(prof.chars_analysed) + ' characters)' + (prof.model ? ' · ' + prof.model : ''));

    const fieldCard = (key) => {
      const f = prof.fields[key];
      const box = h('div', { class: 'afield' + (f.edited ? ' edited' : '') });
      const origin = h('div', { class: 'afield-original', hidden: true });
      const control = h('div', null);
      const drawControl = () => {
        clear(control);
        if (key === 'suggested_tone') {
          const tones = (p.tones || []).map((t) => [t.value, (TONE_ICONS[t.value] || '') + ' ' + t.label]);
          control.appendChild(segmented(tones, draft[key], 'Suggested tone', (v) => { draft[key] = v; refreshState(); }));
        } else if (f.list) {
          const list = draft[key];
          const chips = h('div', { class: 'chip-input' });
          const input = h('input', { type: 'text', placeholder: list.length >= 6 ? 'Six is the most' : 'Add, then Enter', maxlength: '120', disabled: list.length >= 6, 'aria-label': 'Add to ' + f.label });
          list.forEach((item, i) => chips.appendChild(h('span', { class: 'chip' }, h('span', { class: 'chip-text' }, item),
            h('button', { class: 'chip-x', type: 'button', 'aria-label': 'Remove ' + item, onclick: () => { list.splice(i, 1); drawControl(); refreshState(); } }, icon('x')))));
          input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && input.value.trim()) { e.preventDefault(); if (list.length < 6) list.push(input.value.trim()); drawControl(); refreshState(); const again = control.querySelector('input'); if (again) again.focus(); }
          });
          chips.appendChild(input);
          control.appendChild(chips);
        } else {
          const max = key === 'summary' ? 700 : key === 'tone_reason' ? 200 : 400;
          const ta = h('textarea', { class: 'textarea', rows: key === 'summary' ? '4' : '2', maxlength: String(max), 'aria-label': f.label });
          ta.value = draft[key] || '';
          const count = h('span', { class: 'text-count' }, (ta.value.length) + ' / ' + max);
          const grow = () => { ta.style.height = 'auto'; ta.style.height = Math.min(320, ta.scrollHeight + 2) + 'px'; };
          ta.addEventListener('input', () => { draft[key] = ta.value; count.textContent = ta.value.length + ' / ' + max; grow(); refreshState(); });
          requestAnimationFrame(grow);
          append(control, [ta, h('div', { class: 'afield-count' }, count)]);
        }
      };
      drawControl();
      const origValue = f.original;
      const origText = Array.isArray(origValue) ? (origValue.length ? origValue.join(' · ') : '(empty)') : key === 'suggested_tone' ? toneLabel(origValue) : (origValue || '(empty)');
      append(origin, [h('span', { class: 'afield-original-label' }, 'AI original'), h('p', null, origText),
        h('button', { class: 'btn sm ghost', type: 'button', onclick: () => { draft[key] = JSON.parse(JSON.stringify(origValue)); drawControl(); refreshState(); } }, icon('reset'), 'Use the original')]);
      const toggle = h('button', { class: 'btn sm ghost afield-toggle', type: 'button', 'aria-expanded': 'false', onclick: () => { origin.hidden = !origin.hidden; toggle.setAttribute('aria-expanded', String(!origin.hidden)); } }, icon('bot'), 'AI original');
      append(box, [
        h('div', { class: 'afield-head' }, h('span', { class: 'label', style: 'margin:0' }, f.label), f.edited ? h('span', { class: 'badge unsaved' }, 'Edited by mods') : null, h('span', { class: 'grow' }), toggle),
        control, origin]);
      return box;
    };

    const actions = h('div', { class: 'analysis-actions' },
      h('button', { class: 'btn', type: 'button', onclick: async () => {
        const ok = await confirmDialog({ title: 'Analyse ' + p.name + ' again?', icon: 'flask', confirm: 'Analyse again', body: 'A fresh draft replaces this one, including any edits mods made to it. Notes already added stay as they are.' });
        if (ok) reanalyse(true);
      } }, icon('restart'), 'Re-analyse'),
      h('button', { class: 'btn', type: 'button', onclick: async () => {
        try {
          const r = await api('PUT', '/profiles/' + p.id, { status: prof.status === 'reviewed' ? 'draft' : 'reviewed' });
          toast(r.profile.status === 'reviewed' ? 'Marked reviewed' : 'Back to draft'); refreshAudit(); clear(panel); profileAnalysis(panel, p, show);
        } catch (e) { toast(e.message, 'error'); }
      } }, icon(prof.status === 'reviewed' ? 'edit' : 'check'), prof.status === 'reviewed' ? 'Back to draft' : 'Mark reviewed'),
      h('button', { class: 'btn danger', type: 'button', onclick: async () => {
        const ok = await confirmDialog({ title: 'Delete this analysis?', icon: 'trash', danger: true, confirm: 'Delete', body: 'The draft and any edits are removed. Notes already added stay as they are.' });
        if (!ok) return;
        try { await api('DELETE', '/profiles/' + p.id); toast('Deleted the analysis'); refreshAudit(); clear(panel); profileAnalysis(panel, p, show); } catch (e) { toast(e.message, 'error'); }
      } }, icon('trash'), 'Delete'),
      h('span', { class: 'grow' }),
      h('button', { class: 'btn primary', type: 'button', onclick: () => {
        if (dirtyFields().length) { toast('Save your edits first, so the notes get what you see.', 'error'); return; }
        addToNotes(p, prof);
      } }, icon('plus'), 'Add to notes…'));

    saveBtn.addEventListener('click', async () => {
      const edits = {};
      dirtyFields().forEach((k) => { edits[k] = draft[k]; });
      saveBtn.disabled = true;
      try {
        const r = await api('PUT', '/profiles/' + p.id, { edits });
        toast('Saved ' + plural(r.changed.length, 'edit'));
        refreshAudit(); clear(panel); profileAnalysis(panel, p, show);
      } catch (e) { toast(e.message, 'error'); refreshState(); }
    });

    const grid = h('div', { class: 'afields' }, ['summary', 'interests', 'style', 'games', 'vibe_with_bot'].map(fieldCard));
    const tone = h('div', { class: 'afields' }, ['suggested_tone', 'tone_reason', 'roast_material', 'avoid'].map(fieldCard));
    panel.appendChild(h('section', { class: 'card analysis-card' },
      h('div', { class: 'card-head' }, h('div', { class: 'grow' }, h('div', { class: 'title-row' }, h('h2', null, 'Bot’s analysis'), statusChip), meta)),
      h('div', { class: 'card-body pad analysis-body' }, privacy, h('div', { class: 'analysis-tools' }, h('span', null, 'Check the basis:'), seenLink, inputBtn), jobHolder, grid,
        h('h3', { class: 'analysis-sub' }, 'Tone'), tone,
        h('div', { class: 'analysis-save' }, dirtyNote, h('span', { class: 'grow' }), saveBtn), actions)));
    refreshState();
    api('GET', '/profiles/job').then((r) => { activeState.job = r.job; if (r.job && r.job.running && r.job.items.some((i) => i.user_id === p.id)) watchJob(jobHolder, () => { if (panel.isConnected) { clear(panel); profileAnalysis(panel, p, show); } }); }).catch(() => {});
  }

  async function showPrompt(p) {
    let r;
    try { r = await api('GET', '/profiles/' + p.id + '/prompt'); } catch (e) { toast(e.message, 'error'); return; }
    await confirmDialog({ title: 'What the model would read for ' + p.name, icon: 'eye', wide: true, confirm: 'Close', cancel: 'Back',
      body: h('div', { class: 'review' },
        h('p', { class: 'hint', style: 'margin:0' }, plural(r.messages, 'message') + ' · ' + numberFmt.format(r.prompt_chars) + ' characters in all (about ' + numberFmt.format(Math.round(r.prompt_chars / 4)) + ' tokens). #safe-corner, threads in it, DMs, unknown channels and quoted replies are already removed.'),
        h('pre', { class: 'ai-preview prompt-preview' }, r.prompt)) });
  }

  const APPLY_FIELDS = ['summary', 'interests', 'style', 'games', 'vibe_with_bot', 'tone_reason', 'roast_material', 'avoid'];

  async function addToNotes(p, prof) {
    const chosen = new Set(['summary', 'interests', 'style', 'avoid']);
    let tone = true;
    const preview = h('pre', { class: 'ai-preview' });
    const block = h('pre', { class: 'ai-preview' });
    const fit = h('p', { class: 'hint', style: 'margin:0' });
    let seq = 0;
    const refresh = async () => {
      const mine = ++seq;
      if (!chosen.size && !tone) { preview.textContent = 'Pick at least one field, or the tone.'; block.textContent = ''; return; }
      try {
        const r = await api('POST', '/profiles/' + p.id + '/apply/preview', { fields: [...chosen], tone });
        if (mine !== seq) return;
        preview.textContent = r.note || '(no note text)';
        block.textContent = r.context_block || 'Nothing: the note is empty and the tone is normal.';
        fit.className = r.fits ? 'hint' : 'error-text';
        fit.textContent = r.fits ? r.chars + ' / ' + r.max + ' characters' + (r.tone !== (p.note && p.note.tone) && tone ? ' · tone becomes ' + toneLabel(r.tone) : '') : 'Too long by ' + r.over_by + ' characters: pick fewer fields, or trim the note first.';
      } catch (e) { preview.textContent = e.message; }
    };
    const boxes = h('div', { class: 'apply-fields' }, APPLY_FIELDS.map((k) => {
      const f = prof.fields[k];
      const empty = Array.isArray(f.value) ? !f.value.length : !String(f.value || '').trim();
      const box = h('input', { type: 'checkbox', checked: chosen.has(k) && !empty, disabled: empty });
      if (empty) chosen.delete(k);
      box.addEventListener('change', () => { if (box.checked) chosen.add(k); else chosen.delete(k); refresh(); });
      return h('label', { class: 'check' + (empty ? ' muted' : '') }, box, h('span', null, f.label, empty ? h('small', null, 'empty') : null));
    }));
    const toneBox = h('input', { type: 'checkbox', checked: true });
    toneBox.addEventListener('change', () => { tone = toneBox.checked; refresh(); });
    const dirtyWarn = S.guard && S.guard() ? h('p', { class: 'error-text' }, icon('alert'), 'Your unsaved changes in Mods’ notes will be replaced by this.') : null;
    const body = h('div', { class: 'review' },
      h('p', { class: 'hint', style: 'margin:0' }, 'The chosen fields are added to the end of ' + p.name + '’s note under “From the analysis:”. From then on the bot uses them when it answers them.'),
      boxes,
      h('label', { class: 'check' }, toneBox, h('span', null, 'Also set the tone to ' + toneLabel(prof.fields.suggested_tone.value), h('small', null, prof.fields.tone_reason.value || ''))),
      dirtyWarn,
      h('div', null, h('span', { class: 'label' }, 'The note after'), preview, fit),
      h('div', null, h('span', { class: 'label' }, 'What the AI gets'), block));
    refresh();
    const ok = await confirmDialog({ title: 'Add to ' + p.name + '’s notes', icon: 'edit', wide: true, confirm: 'Add to notes', body });
    if (!ok) return;
    try {
      await api('POST', '/profiles/' + p.id + '/apply', { fields: [...chosen], tone });
      toast('Added to the notes for ' + p.name);
      refreshAudit();
      S.guard = null;
      rerender();
    } catch (e) { toast(e.message, 'error'); }
  }

  // --- insights -------------------------------------------------------------------------------

  const INSIGHT_PERIODS = [['today', 'Today'], ['7d', '7 days'], ['30d', '30 days'], ['all', 'All recorded']];
  const insightState = { period: '7d', talkTab: 'magnets', data: null, rebuild: null };

  function who(p) {
    const name = (p && p.name) || 'Former member';
    return h('a', { class: 'who-link', href: '#/members/' + p.id }, name);
  }
  function whoPlain(p) { return (p && p.name) || 'Former member'; }

  function dayPart(ts) {
    const d = new Date((ts + 19800) * 1000);
    const hr = d.getUTCHours();
    const day = new Intl.DateTimeFormat('en-GB', { timeZone: 'Asia/Kolkata', weekday: 'long' }).format(new Date(ts * 1000));
    const part = hr < 5 ? 'late night' : hr < 12 ? 'morning' : hr < 17 ? 'afternoon' : hr < 21 ? 'evening' : 'night';
    return day + ' ' + part;
  }

  function tidbitCards(d) {
    const cards = [];
    const card = (emoji, body, onclick) => h(onclick ? 'button' : 'div', { class: 'tidbit', type: onclick ? 'button' : null, onclick }, h('span', { class: 'tidbit-emoji', 'aria-hidden': 'true' }, emoji), h('p', null, body));
    if (d.longest) {
      const r = d.longest.run;
      cards.push(card('🏓', [h('b', null, whoPlain(r.starter)), ' and ', h('b', null, whoPlain(r.other)), ' had a ', h('b', null, r.len + '-reply'), ' back-and-forth', r.channel ? ' in #' + r.channel : '', ' on ' + dayPart(r.start_ts) + (r.minutes > 1 ? ' (' + r.minutes + ' min)' : '')], () => openPair(r.starter.id, r.other.id)));
    }
    if (d.duos[0]) {
      const p = d.duos[0];
      cards.push(card('👯', [h('b', null, whoPlain(p.a)), ' and ', h('b', null, whoPlain(p.b)), ' replied to each other ', h('b', null, p.replies + ' times'), ', more than any other pair'], () => openPair(p.a.id, p.b.id)));
    }
    if (d.magnets[0]) {
      const m = d.magnets[0];
      cards.push(card('🧲', [h('b', null, whoPlain(m.member)), ' was replied to by ', h('b', null, m.people + ' different people'), ' (' + plural(m.count, 'reply', 'replies') + ')']));
    }
    if (d.one_sided[0]) {
      const o = d.one_sided[0];
      cards.push(card('🙈', [h('b', null, whoPlain(o.from)), ' replied to ', h('b', null, whoPlain(o.to)), ' ' + o.replies + ' times; ' + whoPlain(o.to) + ' replied ' + plural(o.back, 'time')], () => openPair(o.from.id, o.to.id)));
    }
    if (d.mentioned[0]) {
      const m = d.mentioned[0];
      cards.push(card('📣', [h('b', null, whoPlain(m.member)), ' got @-mentioned ', h('b', null, m.count + ' times'), ' by ' + plural(m.people, 'person', 'people')]));
    }
    if (d.arena[0]) {
      const a = d.arena[0];
      const lead = a.a_wins === a.b_wins ? 'level at ' + a.a_wins + '–' + a.b_wins : (a.a_wins > a.b_wins ? whoPlain(a.a) : whoPlain(a.b)) + ' leads ' + Math.max(a.a_wins, a.b_wins) + '–' + Math.min(a.a_wins, a.b_wins);
      cards.push(card('⚔️', [h('b', null, whoPlain(a.a)), ' and ', h('b', null, whoPlain(a.b)), ' fought ', h('b', null, a.fights + ' times'), ' — ' + lead]));
    }
    if (d.night_owls[0]) {
      const n = d.night_owls[0];
      cards.push(card('🦉', [h('b', null, Math.round(n.share * 100) + '%'), ' of ', h('b', null, whoPlain(n.member)), '’s messages come between midnight and 5 am']));
    }
    if (d.new_connections[0]) {
      const p = d.new_connections[0];
      cards.push(card('🌱', [h('b', null, whoPlain(p.a)), ' and ', h('b', null, whoPlain(p.b)), ' started replying to each other ' + (p.first_ts ? ago(p.first_ts) : 'recently') + ' — ' + plural(p.replies, 'reply', 'replies') + ' since'], () => openPair(p.a.id, p.b.id)));
    }
    return cards.slice(0, 6);
  }

  function twoWay(aCount, bCount, max) {
    const total = Math.max(1, max);
    return h('span', { class: 'twoway', 'aria-hidden': 'true' },
      h('span', { class: 'tw-left' }, h('i', { style: 'width:' + Math.round((aCount / total) * 100) + '%' })),
      h('span', { class: 'tw-right' }, h('i', { style: 'width:' + Math.round((bCount / total) * 100) + '%' })));
  }

  function balanceWords(p) {
    if (!p.a_to_b || !p.b_to_a) return 'one way';
    if (p.balance >= 0.75) return 'even';
    return 'mostly ' + whoPlain(p.a_to_b > p.b_to_a ? p.a : p.b);
  }

  function renderInsights(page) {
    document.title = 'Insights · Loduchand';
    const st = insightState;
    page.appendChild(pageHead('Insights', 'Who talks to whom: replies, mentions, the longest back-and-forths and a few fun tidbits.', null, h('span', { class: 'feature-icon', 'aria-hidden': 'true' }, '✨')));
    const body = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Counting replies…'));
    page.appendChild(h('div', { class: 'toolbar' }, segmented(INSIGHT_PERIODS, st.period, 'Period', (v) => { st.period = v; load(); })));
    page.appendChild(body);
    const load = async () => {
      clear(body).appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Counting replies…'));
      try { st.data = await api('GET', '/insights?period=' + st.period); } catch (e) { clear(body).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); return; }
      if (!page.isConnected) return;
      draw();
    };
    const draw = () => {
      const d = st.data;
      clear(body);
      if (!d.counts.replies && !d.counts.mentions) {
        body.appendChild(h('div', { class: 'card empty' }, icon('message'), h('h3', null, 'Nothing counted yet'), h('p', null, 'Replies and mentions between members show up here as they happen.')));
        body.appendChild(dataNote(d));
        return;
      }
      body.appendChild(h('p', { class: 'insight-counts' }, h('b', null, numberFmt.format(d.counts.replies)), ' replies and ', h('b', null, numberFmt.format(d.counts.mentions)), ' mentions between ', h('b', null, numberFmt.format(d.counts.people)), ' members'));
      body.appendChild(h('div', { class: 'tidbits' }, tidbitCards(d)));

      // top duos
      const maxDir = Math.max(1, ...d.duos.map((p) => Math.max(p.a_to_b, p.b_to_a)));
      const duoList = h('ol', { class: 'duo-list' }, d.duos.map((p, i) => h('li', null, h('button', { type: 'button', class: 'duo', onclick: () => openPair(p.a.id, p.b.id) },
        h('span', { class: 'sr-rank' }, i + 1),
        h('span', { class: 'duo-names' }, avatar(p.a.avatar, whoPlain(p.a), 'xs'), h('b', null, whoPlain(p.a)), h('span', { class: 'duo-amp' }, '⇄'), avatar(p.b.avatar, whoPlain(p.b), 'xs'), h('b', null, whoPlain(p.b))),
        h('span', { class: 'duo-bar' }, h('small', null, p.a_to_b), twoWay(p.a_to_b, p.b_to_a, maxDir), h('small', null, p.b_to_a)),
        h('span', { class: 'duo-meta' }, h('span', { class: 'badge' + (p.balance >= 0.75 ? ' on' : '') }, balanceWords(p)),
          p.longest ? h('span', { class: 'duo-run', title: 'Longest back-and-forth' }, '🏓 ' + p.longest.len + ' in a row') : null,
          p.top_channel && p.top_channel.name ? h('span', { class: 'duo-ch' }, '#' + p.top_channel.name) : null),
        h('span', { class: 'duo-last' }, ago(p.last_ts))))));
      body.appendChild(card('ins-duos', 'Top duos', 'Replies both ways · click a pair for the detail', h('div', { class: 'card-body' }, duoList)));

      // who talks most
      const talkLists = { magnets: ['Replied to', d.magnets, (m) => plural(m.people, 'person', 'people') + ' · ' + plural(m.count, 'reply', 'replies')], replies_sent: ['Replies sent', d.replies_sent, (m) => plural(m.count, 'reply', 'replies') + ' to ' + plural(m.people, 'person', 'people')], mentions_sent: ['Mentions sent', d.mentions_sent, (m) => plural(m.count, 'mention') + ' of ' + plural(m.people, 'person', 'people')], mentioned: ['Mentioned', d.mentioned, (m) => plural(m.count, 'mention') + ' from ' + plural(m.people, 'person', 'people')] };
      const talkBody = h('div', null);
      const drawTalk = () => {
        clear(talkBody);
        const [, list, words] = talkLists[st.talkTab];
        const max = Math.max(1, ...list.map((m) => (st.talkTab === 'magnets' ? m.people : m.count)));
        talkBody.appendChild(list.length ? h('ol', { class: 'rank-list' }, list.map((m, i) => h('li', null, h('span', { class: 'sr-rank' }, i + 1), avatar(m.member.avatar, whoPlain(m.member), 'xs'), who(m.member),
          h('span', { class: 'hb-track' }, h('i', { style: 'width:' + Math.round(((st.talkTab === 'magnets' ? m.people : m.count) / max) * 100) + '%' })), h('small', null, words(m))))) : h('p', { class: 'empty-small', style: 'padding:14px 16px' }, 'Nothing yet.'));
      };
      drawTalk();
      const talkCard = card('ins-talk', 'Who talks most', null, h('div', { class: 'card-body' }, h('div', { class: 'talk-tabs' }, segmented(Object.keys(talkLists).map((k) => [k, talkLists[k][0]]), st.talkTab, 'List', (v) => { st.talkTab = v; drawTalk(); })), talkBody));

      const oneSided = card('ins-one', 'One-sided', 'At least 15 replies one way, a fifth or fewer back', h('div', { class: 'card-body' }, d.one_sided.length ? h('ul', { class: 'plain-list' }, d.one_sided.map((o) => h('li', null, h('button', { type: 'button', class: 'line-btn', onclick: () => openPair(o.from.id, o.to.id) },
        h('span', null, h('b', null, whoPlain(o.from)), ' keeps replying to ', h('b', null, whoPlain(o.to))), h('small', null, o.replies + ' → · ' + o.back + ' ←'))))) : h('p', { class: 'empty-small', style: 'padding:14px 16px' }, 'Nobody is being left on read this period.')));

      const arena = card('ins-arena', 'Arena rivalries', '1v1 fights, head to head', h('div', { class: 'card-body' }, d.arena.length ? h('ul', { class: 'plain-list' }, d.arena.map((a) => h('li', { class: 'rival' },
        h('span', { class: 'rival-name left' }, who(a.a), h('b', null, a.a_wins)),
        h('span', { class: 'rival-bar', 'aria-label': a.a_wins + ' to ' + a.b_wins }, h('i', { class: 'l', style: 'flex-grow:' + Math.max(0.2, a.a_wins) }), h('i', { class: 'r', style: 'flex-grow:' + Math.max(0.2, a.b_wins) })),
        h('span', { class: 'rival-name right' }, h('b', null, a.b_wins), who(a.b))))) : h('p', { class: 'empty-small', style: 'padding:14px 16px' }, 'No fights this period.')));
      const hoursList = (list, label) => list.length ? h('ol', { class: 'rank-list' }, list.map((m, i) => h('li', null, h('span', { class: 'sr-rank' }, i + 1), avatar(m.member.avatar, whoPlain(m.member), 'xs'), who(m.member),
        h('span', { class: 'hb-track' }, h('i', { style: 'width:' + Math.round(m.share * 100) + '%' })), h('small', null, Math.round(m.share * 100) + '% ' + label)))) : h('p', { class: 'empty-small', style: 'padding:14px 16px' }, 'Nobody with 100+ messages yet.');
      const owls = card('ins-owls', 'Night owls & early birds', 'Share of messages, India time, 100+ messages', h('div', { class: 'card-body owls' },
        h('div', null, h('h4', { class: 'mini-title', style: 'padding:12px 16px 0' }, '🦉 00:00–05:00'), hoursList(d.night_owls, 'at night')),
        h('div', null, h('h4', { class: 'mini-title', style: 'padding:12px 16px 0' }, '🐦 05:00–09:00'), hoursList(d.early_birds, 'early'))));
      body.appendChild(h('div', { class: 'two-col ins-cols' }, talkCard, h('div', { class: 'ins-stack' }, oneSided, arena)));
      body.appendChild(owls);

      if (d.period !== 'all') {
        body.appendChild(card('ins-new', 'New connections', 'Pairs whose first reply to each other came in this period' + (d.coverage.earliest ? ', since counting began ' + fmtDate(d.coverage.earliest * 1000) : ''),
          h('div', { class: 'card-body' }, d.new_connections.length ? h('ul', { class: 'plain-list' }, d.new_connections.map((p) => h('li', null, h('button', { type: 'button', class: 'line-btn', onclick: () => openPair(p.a.id, p.b.id) },
            h('span', null, '🌱 ', h('b', null, whoPlain(p.a)), ' & ', h('b', null, whoPlain(p.b))), h('small', null, plural(p.replies, 'reply', 'replies') + ' · first ' + (p.first_ts ? fmtDate(p.first_ts * 1000) : '')))))) : h('p', { class: 'empty-small', style: 'padding:14px 16px' }, 'No new pairs this period.'))));
      }
      body.appendChild(dataNote(d));
    };
    load();
  }

  function dataNote(d) {
    const holder = h('div', { class: 'data-note' });
    const since = d.coverage.earliest ? fmtDate(d.coverage.earliest * 1000) : 'the start';
    const liveSince = d.coverage.live_since ? fmtDate(d.coverage.live_since * 1000) : null;
    const status = h('span', { class: 'rebuild-status' });
    const btn = h('button', { class: 'btn sm', type: 'button' }, icon('restart'), 'Rebuild from history');
    const poll = async () => {
      try {
        const r = await api('GET', '/insights/rebuild');
        const s = r.rebuild;
        clear(status);
        if (s.running) { append(status, [h('span', { class: 'spinner' }), ' Reading stored messages… ' + numberFmt.format(s.scanned)]); btn.disabled = true; setTimeout(() => { if (holder.isConnected) poll(); }, 1500); }
        else if (s.finished_ts) { btn.disabled = false; append(status, s.error ? h('span', { class: 'error-text' }, 'Last rebuild failed: ' + s.error) : 'Last rebuilt ' + ago(s.finished_ts) + ' · ' + numberFmt.format(s.added) + ' replies and mentions from ' + numberFmt.format(s.scanned) + ' stored messages'); }
      } catch (_) { /* ignore */ }
    };
    btn.addEventListener('click', async () => {
      const ok = await confirmDialog({ title: 'Rebuild from the stored history?', icon: 'restart', confirm: 'Rebuild',
        body: 'Replies are read again from the bot’s stored messages of the last ' + d.coverage.retention_days + ' days and replace the earlier fill-in. Counts recorded live stay as they are. It runs in the background and takes a minute or so.' });
      if (!ok) return;
      try { await api('POST', '/insights/rebuild'); toast('Rebuilding in the background', 'info'); poll(); } catch (e) { toast(e.message, 'error'); }
    });
    append(holder, [icon('info'), h('div', { class: 'grow' },
      h('p', null, 'Counts replies and mentions between members in channels the bot can see; #safe-corner and DMs are never counted. No message text is kept. ' +
        (liveSince ? 'History before ' + liveSince + ' is filled in from the bot’s stored messages, so quieter channels may be under-counted. ' : '') + 'Counted since ' + since + '; older than ' + d.coverage.retention_days + ' days is dropped.'),
      h('div', { class: 'rebuild-row' }, btn, status))]);
    poll();
    return holder;
  }

  /** A read-only side panel. */
  function openSheet(title, sub, onClose) {
    const before = document.activeElement;
    const body = h('div', { class: 'drawer-body' });
    const drawer = h('div', { class: 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('div', { class: 'drawer-head' }, h('div', { class: 'grow' }, h('h2', null, title), sub ? h('div', { class: 'sub' }, sub) : null),
        h('button', { class: 'btn ghost icon-only', type: 'button', 'aria-label': 'Close', onclick: () => close() }, icon('x'))), body);
    const scrim = h('div', { class: 'scrim', onclick: () => close() });
    const close = () => { drawer.remove(); scrim.remove(); popLayer(layer); if (before && before.isConnected && before.focus) before.focus(); if (onClose) onClose(); };
    const layer = pushLayer({ drawer: true, close });
    drawer.addEventListener('keydown', (e) => { if (e.key === 'Tab') trapFocus(drawer, e); });
    $('#layers').appendChild(scrim);
    $('#layers').appendChild(drawer);
    return { body, close, drawer };
  }

  async function openPair(a, b, period) {
    period = period || insightState.period || '30d';
    const sheet = openSheet('Pair detail', INSIGHT_PERIODS.find((p) => p[0] === period)[1]);
    sheet.body.appendChild(h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Loading…'));
    let d;
    try { d = await api('GET', '/insights/pair?a=' + a + '&b=' + b + '&period=' + period); } catch (e) { clear(sheet.body).appendChild(h('p', { class: 'error-text' }, e.message)); return; }
    clear(sheet.body);
    const s = d.summary || { a_to_b: 0, b_to_a: 0, mentions_a_to_b: 0, mentions_b_to_a: 0, longest: null };
    const nameA = whoPlain(d.a), nameB = whoPlain(d.b);
    sheet.drawer.querySelector('.drawer-head h2').textContent = nameA + ' ⇄ ' + nameB;
    const periodSwitch = segmented(INSIGHT_PERIODS, period, 'Period', (v) => { sheet.close(); openPair(a, b, v); });
    const maxDay = Math.max(1, ...d.days.map((x) => Math.max(x.a_to_b, x.b_to_a)));
    const chart = h('div', { class: 'mirror', role: 'img', 'aria-label': 'Replies per day each way' }, d.days.map((x) => {
      const col = h('span', { class: 'mirror-col' },
        h('span', { class: 'mirror-up' }, h('i', { style: 'height:' + Math.round((x.a_to_b / maxDay) * 100) + '%' })),
        h('span', { class: 'mirror-down' }, h('i', { style: 'height:' + Math.round((x.b_to_a / maxDay) * 100) + '%' })));
      col.addEventListener('mousemove', (e) => showTip(e, [fmtDay(x.day), nameA + ' → ' + nameB + ': ' + x.a_to_b, nameB + ' → ' + nameA + ': ' + x.b_to_a]));
      col.addEventListener('mouseleave', hideTip);
      return col;
    }));
    const chMax = Math.max(1, ...d.channels.map((c) => c.replies));
    append(sheet.body, [
      h('div', { class: 'pair-head' }, h('span', { class: 'pair-person' }, avatar(d.a.avatar, nameA, 'lg'), who(d.a)), h('span', { class: 'duo-amp big' }, '⇄'), h('span', { class: 'pair-person' }, avatar(d.b.avatar, nameB, 'lg'), who(d.b))),
      h('div', null, periodSwitch),
      h('div', { class: 'stat-row four' },
        stat(nameA + ' → ' + nameB, String(s.a_to_b), 'replies'),
        stat(nameB + ' → ' + nameA, String(s.b_to_a), 'replies'),
        stat('Mentions', String(s.mentions_a_to_b + s.mentions_b_to_a), s.mentions_a_to_b + ' · ' + s.mentions_b_to_a),
        stat('Arena', d.arena.a_wins + '–' + d.arena.b_wins, 'head to head')),
      s.longest ? h('p', { class: 'pair-run' }, '🏓 Longest back-and-forth: ', h('b', null, s.longest.len + ' replies'), (s.longest.channel ? ' in #' + s.longest.channel : '') + ', ' + dayPart(s.longest.start_ts) + ' ' + fmtDate(s.longest.start_ts * 1000) + (s.longest.minutes > 1 ? ' · ' + s.longest.minutes + ' min' : '')) : null,
      h('section', { class: 'form-card' }, h('h3', null, icon('chart'), 'Replies per day'),
        d.days.length ? [h('div', { class: 'legend' }, h('span', { class: 'legend-item' }, h('i', { style: 'background:var(--s1)' }), nameA + ' → ' + nameB), h('span', { class: 'legend-item' }, h('i', { style: 'background:var(--s2)' }), nameB + ' → ' + nameA)), chart,
          h('div', { class: 'range-scale' }, h('span', null, fmtDay(d.days[0].day)), h('span', null, fmtDay(d.days[d.days.length - 1].day)))] : h('p', { class: 'empty-small' }, 'No replies this period.')),
      d.channels.length ? h('section', { class: 'form-card' }, h('h3', null, icon('hash'), 'Where'), h('ul', { class: 'hbars compact' }, d.channels.map((c) => h('li', null, h('span', { class: 'hb-label' }, '#' + (c.name || 'unknown')), h('span', { class: 'hb-track' }, h('i', { style: 'width:' + Math.round((c.replies / chMax) * 100) + '%' })), h('b', null, c.replies))))) : null,
      h('section', { class: 'form-card' }, h('h3', null, icon('clock'), 'Latest exchanges', h('span', { class: 'right hint', style: 'margin:0' }, 'times and places only')),
        d.recent.length ? h('ul', { class: 'exchanges' }, d.recent.map((e) => h('li', null, h('small', null, exchangeTime(e.ts)), h('span', null, h('b', null, e.from === d.a.id ? nameA : nameB), ' → ', e.to === d.a.id ? nameA : nameB), h('span', { class: 'badge' }, e.kind === 'mention' ? '@ mention' : 'reply'), e.channel ? h('small', { class: 'ex-ch' }, '#' + e.channel) : null))) : h('p', { class: 'empty-small' }, 'Nothing yet.')),
    ]);
  }

  function exchangeTime(ts) { return new Intl.DateTimeFormat('en-GB', { timeZone: 'Asia/Kolkata', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hour12: false }).format(new Date(ts * 1000)); }

  async function profileConnections(panel, p) {
    const periodHolder = h('div', { class: 'toolbar', style: 'margin-bottom:12px' });
    const list = h('div', null, h('div', { class: 'cup-loading' }, h('span', { class: 'spinner' }), 'Counting…'));
    let period = insightState.period === 'today' ? '30d' : insightState.period;
    const load = async () => {
      let d;
      try { d = await api('GET', '/members/' + p.id + '/connections?period=' + period); } catch (e) { clear(list).appendChild(h('div', { class: 'card empty' }, h('p', null, e.message))); return; }
      clear(list);
      if (!d.partners.length) { list.appendChild(h('div', { class: 'card empty' }, icon('users'), h('h3', null, 'No connections counted'), h('p', null, 'Replies and mentions with other members show up here.'))); return; }
      const max = Math.max(1, ...d.partners.map((x) => Math.max(x.to_them, x.from_them)));
      list.appendChild(card('pf-conn', 'Connections', p.name + ' sent ' + plural(d.sent, 'reply', 'replies') + ' and got ' + d.received + ' back · click a row for the pair', h('div', { class: 'card-body' },
        h('ul', { class: 'conn-list' }, d.partners.map((x) => h('li', null, h('button', { type: 'button', class: 'conn', onclick: () => openPair(p.id, x.member.id, period) },
          avatar(x.member.avatar, whoPlain(x.member), 'lg'),
          h('span', { class: 'conn-main' }, h('b', null, whoPlain(x.member)),
            h('span', { class: 'conn-bar' }, h('small', { title: 'Replies to them' }, x.to_them + ' →'), twoWay(x.to_them, x.from_them, max), h('small', { title: 'Replies from them' }, '← ' + x.from_them)),
            h('span', { class: 'conn-facts' },
              x.mentions_to_them || x.mentions_from_them ? h('span', null, '@ ' + x.mentions_to_them + ' · ' + x.mentions_from_them) : null,
              x.longest ? h('span', null, '🏓 ' + x.longest.len + ' in a row') : null,
              x.arena.wins || x.arena.losses ? h('span', null, '⚔️ ' + x.arena.wins + '–' + x.arena.losses) : null,
              h('span', { class: 'muted' }, ago(x.last_ts)))),
          icon('right'))))))));
    };
    periodHolder.appendChild(segmented(INSIGHT_PERIODS.filter((x) => x[0] !== 'today'), period, 'Period', (v) => { period = v; load(); }));
    append(panel, [periodHolder, list]);
    load();
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
