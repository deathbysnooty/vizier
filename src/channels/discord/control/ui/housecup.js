// The House Cup scoreboard, drawn from the state beside it.
//
// It reads the page's token out of the address bar, asks `<page>/state` for
// everything, and then asks again every ten seconds. Nothing is ever reloaded:
// the nodes that already exist are edited in place, and a list is only rebuilt
// when its contents actually changed, so a page left open on a second screen
// never flashes and never loses where you had scrolled to.
//
// Nothing on this page names anyone but by their display name, and there is no
// id in the state to name them by anything else.

'use strict';

const parts = window.location.pathname.split('/').filter(Boolean);
const BASE = '/housecup/' + encodeURIComponent(parts[1] || '') + '/state';
const EVERY = 10000;

// --- light or dark ----------------------------------------------------------------
//
// Dark unless the reader's machine asks for light, unless they pressed the
// switch, unless the address says otherwise - in that order. The choice is kept
// in this browser only; nothing about it is ever sent anywhere.

const THEME_KEY = 'housecup:theme';

function kept(key) {
  try {
    return window.localStorage.getItem(key);
  } catch (err) {
    return null;
  }
}

function keep(key, value) {
  try {
    window.localStorage.setItem(key, value);
  } catch (err) {
    /* a private window, or storage turned off: the page works either way */
  }
}

function wear(theme) {
  document.documentElement.dataset.theme = theme;
  const button = document.getElementById('theme');
  if (button) button.textContent = theme === 'light' ? '☀️' : '🌙';
}

const asked = new URLSearchParams(window.location.search).get('theme');
const chosen = asked === 'light' || asked === 'dark' ? asked : kept(THEME_KEY);
const machine = window.matchMedia && window.matchMedia('(prefers-color-scheme: light)');
wear(chosen || (machine && machine.matches ? 'light' : 'dark'));
if (!chosen && machine && machine.addEventListener) {
  machine.addEventListener('change', (event) => wear(event.matches ? 'light' : 'dark'));
}

const el = (id) => document.getElementById(id);
el('theme').addEventListener('click', () => {
  const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
  wear(next);
  keep(THEME_KEY, next);
});

const standings = el('standings');
const houses = el('houses');
const live = el('live');
const freshness = el('freshness');
const trouble = el('trouble');

let state = null;
// The difference between this machine's clock and the server's, so "20s ago" is
// the truth even on a laptop whose clock is minutes out.
let skew = 0;
let lastOk = 0;
let failures = 0;
const built = new Map();

function node(tag, className, text) {
  const out = document.createElement(tag);
  if (className) out.className = className;
  if (text !== undefined) out.textContent = text;
  return out;
}

function say(text) {
  if (!text) {
    trouble.hidden = true;
    trouble.textContent = '';
    return;
  }
  trouble.textContent = text;
  trouble.hidden = false;
}

function thousands(n) {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

function plural(n, one, many) {
  return thousands(n) + ' ' + (n === 1 ? one : many);
}

/** A house's two colours, as custom properties on one element. */
function paint(target, house) {
  target.style.setProperty('--house', house.colour);
  target.style.setProperty('--house-2', house.secondary);
}

const MEDALS = ['🥇', '🥈', '🥉'];

// --- the standings ---------------------------------------------------------------

function standingRow(house) {
  const row = node('div', 'stand');
  row.dataset.key = house.key;
  paint(row, house);
  const place = node('span', 'place');
  const who = node('div', 'who');
  who.append(node('span', 'crest'), node('span', 'name'), node('span', 'gap'));
  const score = node('div', 'score');
  score.append(node('b'), node('span'));
  const track = node('div', 'track');
  const fill = node('div', 'fill');
  track.appendChild(fill);
  row.append(place, who, score, track);
  return row;
}

function drawStandings() {
  for (const house of state.houses) {
    let row = standings.querySelector('.stand[data-key="' + house.key + '"]');
    if (!row) {
      row = standingRow(house);
      standings.appendChild(row);
    }
    // appendChild MOVES a node that is already here, so the order follows the
    // table without anything being thrown away and rebuilt.
    standings.appendChild(row);
    paint(row, house);
    row.classList.toggle('lead', house.rank === 1);
    row.querySelector('.place').textContent = MEDALS[house.rank - 1] || house.rank + '.';
    row.querySelector('.crest').textContent = house.crest;
    row.querySelector('.name').textContent = house.name;
    row.querySelector('.gap').textContent = house.gap > 0 ? thousands(house.gap) + ' behind' : 'in the lead';
    row.querySelector('.score b').textContent = thousands(house.total);
    row.querySelector('.score span').textContent = house.total === 1 ? 'point' : 'points';
    row.querySelector('.fill').style.width = Math.max(0, Math.min(1, house.share)) * 100 + '%';
  }
}

// --- one house -------------------------------------------------------------------

function houseCard(house) {
  const card = node('section', 'house');
  card.dataset.key = house.key;

  const head = node('header');
  head.append(node('span', 'crest'), node('h2'), node('span', 'pts'));

  const scorers = node('div', 'panel');
  const scorersHead = node('h3');
  scorersHead.append(node('span', null, 'Top scorers'), node('em'));
  const list = node('ol', 'scorers');
  scorers.append(scorersHead, list);

  const cards = node('div', 'panel');
  const cardsHead = node('h3');
  cardsHead.append(node('span', null, 'Chocolate Frog cards'), node('em'));
  const grid = node('div', 'cards');
  const holders = node('ol', 'holders');
  const more = node('p', 'more');
  cards.append(cardsHead, grid, node('h3', 'sub'), holders, more);
  cards.querySelector('.sub').append(node('span', null, 'Collectors'), node('em'));

  card.append(head, scorers, cards);
  return card;
}

/** Rebuilds a list only when what it should say has changed. */
function fill(list, rows, signature, make) {
  if (list.dataset.sig === signature) return;
  list.dataset.sig = signature;
  list.textContent = '';
  rows.forEach((row, i) => list.appendChild(make(row, i)));
}

/** A name that opens that member's cards. */
function pick(who, name) {
  const button = node('button', 'pick', name);
  button.type = 'button';
  button.dataset.who = who || '';
  button.setAttribute('aria-haspopup', 'dialog');
  if (!who) {
    // Nothing to show: the state didn't carry this one. Never happens, but a
    // dead button that looks alive would be worse than a plain name.
    button.disabled = true;
    button.classList.add('flat');
  }
  return button;
}

function scorerRow(scorer, i) {
  const li = node('li');
  const rank = node('span', 'rank' + (i < 3 ? ' medal' : ''), MEDALS[i] || String(i + 1));
  const name = node('span', 'nm');
  name.appendChild(pick(scorer.who, scorer.name));
  li.append(rank, name, node('span', 'pt', thousands(scorer.points)));
  return li;
}

function cardChip(card) {
  const chip = node('div', 'card' + (card.held ? '' : ' none'));
  chip.title = card.name + ' · ' + card.rarity;
  chip.append(node('span', 'ce', card.emoji), node('span', 'cn', card.name), node('span', 'cc', card.held ? '×' + card.held : 'missing'));
  return chip;
}

function holderRow(holder, types) {
  const li = node('li');
  const name = node('span', 'nm');
  name.appendChild(pick(holder.who, holder.name));
  if (holder.full_set) {
    const badge = node('span', 'set', 'full set');
    badge.title = 'Holds all ' + types + ' cards, so they can /sellset on their own';
    name.appendChild(badge);
  }
  const count = node('span', 'ct');
  count.appendChild(node('b', null, thousands(holder.cards)));
  count.appendChild(document.createTextNode(' ' + (holder.cards === 1 ? 'card' : 'cards') + ' · ' + holder.types + '/' + types));
  li.append(name, count);
  return li;
}

function drawHouses() {
  const types = state.types.length;
  for (const house of state.houses) {
    let card = built.get(house.key);
    if (!card) {
      card = houseCard(house);
      built.set(house.key, card);
      houses.appendChild(card);
    }
    houses.appendChild(card);
    paint(card, house);
    card.querySelector('header .crest').textContent = house.crest;
    card.querySelector('header h2').textContent = house.name;
    card.querySelector('header .pts').textContent = thousands(house.total);

    const scorers = card.querySelector('.scorers');
    scorers.previousElementSibling.querySelector('em').textContent = house.top.length ? 'this month' : '';
    fill(scorers, house.top, JSON.stringify(house.top), scorerRow);
    let blank = card.querySelector('.panel .empty.scored');
    if (!house.top.length && !blank) {
      blank = node('p', 'empty scored', 'Nobody has scored for this house yet this month.');
      scorers.after(blank);
    } else if (house.top.length && blank) {
      blank.remove();
    }

    const grid = card.querySelector('.cards');
    grid.previousElementSibling.querySelector('em').textContent = house.cards
      ? plural(house.cards, 'card', 'cards') + ' held · ' + (types - house.missing) + ' of ' + types + ' kinds'
      : 'none held yet';
    fill(grid, house.held, JSON.stringify(house.held), cardChip);

    const holders = card.querySelector('.holders');
    holders.previousElementSibling.querySelector('em').textContent =
      house.full_sets ? plural(house.full_sets, 'full set', 'full sets') : '';
    if (house.collectors.length) {
      fill(holders, house.collectors, JSON.stringify(house.collectors), (row) => holderRow(row, types));
    } else {
      fill(holders, [{ empty: true }], 'empty', () => node('li', 'empty', 'Nobody in this house is holding a card yet.'));
    }
    const more = card.querySelector('.more');
    more.textContent = house.more_collectors ? 'and ' + plural(house.more_collectors, 'other collector', 'other collectors') : '';
    more.hidden = !house.more_collectors;
  }
}

// --- one member's cards ----------------------------------------------------------
//
// Clicking a name opens this. It is filled from the same state as everything
// else, and redrawn in place on every refresh, so the ten-second update goes on
// underneath it rather than shutting it or leaving it stale. Nothing is scrolled
// or moved while it is open, so closing it leaves the page exactly where it was.

const veil = el('veil');
const sheet = el('sheet');

/** handle -> member, rebuilt from each state. */
let people = new Map();
/** The handle whose cards are open, or null. */
let showing = null;

function ordinal(n) {
  const tens = n % 100;
  const suffix = tens >= 11 && tens <= 13 ? 'th' : ['th', 'st', 'nd', 'rd'][n % 10] || 'th';
  return n + suffix;
}

function houseOf(key) {
  return state && state.houses.find((h) => h.key === key);
}

function memberCard(card) {
  const copies = (card.serials || []).length;
  const chip = node('div', 'card mine');
  chip.title = card.name + ' · ' + (card.rarity_name || card.rarity);
  const body = node('span', 'cn');
  body.appendChild(node('span', 'ck', card.name));
  body.appendChild(node('span', 'sr', card.serials.map((s) => '#' + s).join(' · ')));
  chip.append(node('span', 'ce', card.emoji), body);
  if (copies > 1) chip.appendChild(node('span', 'cc', '×' + copies));
  return chip;
}

function drawSheet() {
  const member = people.get(showing);
  // Somebody who has slipped off every list the page draws keeps the cards they
  // had a moment ago rather than having the panel blank or close under a reader.
  if (!member) return;
  const home = houseOf(member.house);
  if (home) paint(sheet, home);
  el('sheet-crest').textContent = home ? home.crest : '';
  el('sheet-name').textContent = member.name;

  const where = [];
  where.push(home ? home.name : '');
  where.push(member.points ? plural(member.points, 'point', 'points') + ' this month' : 'no points yet this month');
  if (member.place) where.push(ordinal(member.place) + ' in the house');
  el('sheet-sub').textContent = where.filter(Boolean).join(' · ');

  const types = state.types.length;
  const line = el('sheet-line');
  line.textContent = '';
  if (member.held) {
    line.appendChild(node('b', null, plural(member.held, 'card', 'cards')));
    line.appendChild(document.createTextNode(' · ' + member.types + ' of ' + types + ' kinds'));
    if (member.full_set) {
      const badge = node('span', 'set', 'full set');
      badge.title = 'Holds all ' + types + ' cards, so they can /sellset on their own';
      line.appendChild(badge);
    }
  } else {
    line.appendChild(node('b', null, 'No cards yet'));
  }

  const grid = el('sheet-cards');
  fill(grid, member.cards, member.who + ':' + JSON.stringify(member.cards), memberCard);
  grid.hidden = !member.cards.length;
  const none = el('sheet-none');
  none.textContent = member.cards.length
    ? ''
    : 'They are holding no Chocolate Frog cards at the moment. Cards are caught with /frog in Discord.';
  none.hidden = !!member.cards.length;
}

function opener() {
  return Array.prototype.find.call(document.querySelectorAll('.pick'), (b) => b.dataset.who === showing);
}

function reveal(who) {
  if (!people.has(who)) return;
  showing = who;
  drawSheet();
  veil.hidden = false;
  const was = opener();
  if (was) was.setAttribute('aria-expanded', 'true');
  // preventScroll, here and on the way back: taking the focus must never be the
  // thing that moves the page out from under the reader.
  sheet.focus({ preventScroll: true });
}

function shut() {
  if (showing === null) return;
  // The button may have been rebuilt by a refresh, so it is found again by hand
  // rather than held on to: whoever is standing in its place takes the focus.
  const back = opener();
  showing = null;
  veil.hidden = true;
  document.querySelectorAll('.pick[aria-expanded]').forEach((b) => b.removeAttribute('aria-expanded'));
  if (back) back.focus({ preventScroll: true });
}

document.addEventListener('click', (event) => {
  const button = event.target.closest && event.target.closest('.pick');
  if (button && button.dataset.who) {
    event.preventDefault();
    reveal(button.dataset.who);
  }
});

el('shut').addEventListener('click', shut);
// A click on the dark, never one inside the panel itself.
veil.addEventListener('click', (event) => {
  if (event.target === veil) shut();
});
document.addEventListener('keydown', (event) => {
  if (showing === null) return;
  if (event.key === 'Escape') {
    event.preventDefault();
    shut();
    return;
  }
  if (event.key !== 'Tab') return;
  // Keep the keyboard inside the panel while it is up.
  const stops = sheet.querySelectorAll('button, [href], [tabindex]:not([tabindex="-1"])');
  if (!stops.length) return;
  const first = stops[0];
  const last = stops[stops.length - 1];
  if (event.shiftKey && (document.activeElement === first || document.activeElement === sheet)) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
});
// The page behind is never scrolled or locked - it is simply left alone - so a
// wheel or a drag on the dark moves nothing and closing gives back the exact
// view the reader had.
for (const kind of ['wheel', 'touchmove']) {
  veil.addEventListener(kind, (event) => {
    if (event.target === veil) event.preventDefault();
  }, { passive: false });
}

// --- how fresh it is -------------------------------------------------------------

function ago(seconds) {
  if (seconds < 10) return 'updated just now';
  if (seconds < 60) return 'updated ' + Math.round(seconds) + 's ago';
  if (seconds < 3600) return 'updated ' + Math.round(seconds / 60) + 'm ago';
  return 'updated ' + Math.round(seconds / 3600) + 'h ago';
}

function tick() {
  if (!state) return;
  const age = Math.max(0, Date.now() / 1000 - skew - state.generated);
  freshness.textContent = ago(age);
  live.classList.toggle('stale', failures > 0 || age > 60);
  live.classList.remove('gone');
}

// --- talking to the server -------------------------------------------------------

function draw() {
  el('period').textContent =
    'This month · ' + state.month.label + ' · counted from midnight on the 1st, India time';
  people = new Map((state.members || []).map((m) => [m.who, m]));
  drawStandings();
  drawHouses();
  if (showing !== null) drawSheet();
  tick();
}

async function refresh() {
  try {
    const res = await fetch(BASE, { headers: { accept: 'application/json' } });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      failures++;
      say(body.error || 'The scoreboard isn’t available right now.');
      live.classList.add('gone');
      freshness.textContent = 'not updating';
      return;
    }
    skew = Date.now() / 1000 - body.now;
    state = body;
    lastOk = Date.now();
    failures = 0;
    say('');
    draw();
  } catch (err) {
    failures++;
    // A phone that slept, or a dropped wifi: it keeps what is on screen and
    // says how old it is rather than blanking the page.
    if (failures > 1) say('Can’t reach the server just now — this is the last scoreboard it sent. It keeps trying.');
    tick();
  }
}

refresh();
window.setInterval(() => {
  // A hidden tab costs the server nothing; it catches up the moment it is looked at.
  if (document.visibilityState === 'visible') refresh();
}, EVERY);
window.setInterval(tick, 1000);
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && Date.now() - lastOk > EVERY) refresh();
});
