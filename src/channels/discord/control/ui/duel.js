// The private Letter Duel board page.
//
// It reads the game and the link out of the address bar, asks the server what
// the board and its own rack are, and draws them. Tiles are placed by tapping:
// a tile, then a square. Nothing is sent until Play is pressed — before that
// the server is only ASKED what the play would score, and the answer is what
// the page shows. The server checks everything again when the play lands, so
// this file only has to be pleasant, not trustworthy. Nothing is kept in the
// browser: close the tab and no trace of the link is left behind.

'use strict';

const parts = window.location.pathname.split('/').filter(Boolean);
const GAME = parts[1] || '';
const TOKEN = parts[2] || '';
const BASE = '/duel/' + encodeURIComponent(GAME) + '/' + encodeURIComponent(TOKEN);
const SIZE = 15;
const ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ';
// What each letter is worth, so the page can label tiles without asking.
const VALUES = { A: 1, B: 3, C: 3, D: 2, E: 1, F: 4, G: 2, H: 4, I: 1, J: 8, K: 5, L: 1, M: 3, N: 1, O: 1, P: 3, Q: 10, R: 1, S: 1, T: 1, U: 1, V: 4, W: 4, X: 8, Y: 4, Z: 10 };

const el = (id) => document.getElementById(id);
const board = el('board');
const rackBox = el('rack');
const note = el('note');
const picker = el('picker');

let state = null;
/** Tiles put down this turn but not yet played: {at, letter, blank, from}. */
let pending = [];
/** The rack slot the player has tapped, as an index into the rack string. */
let picked = null;
/** Rack slots chosen for an exchange, as indexes. */
let swapping = [];
/** True while the rack is a chooser for an exchange rather than for placing. */
let swapMode = false;
/** A blank waiting to be told which letter it is: {at, from}. */
let asking = null;
let busy = false;
let timer = null;
let previewing = 0;

function say(text, kind) {
  note.textContent = text || '';
  note.className = 'note' + (kind ? ' ' + kind : '');
}

// --- what the board looks like right now --------------------------------------

/** The board as characters, with this turn's un-played tiles laid on top. */
function cells() {
  const out = String((state && state.board) || '').padEnd(SIZE * SIZE, '.').slice(0, SIZE * SIZE).split('');
  for (const p of pending) out[p.at] = p.blank ? p.letter.toLowerCase() : p.letter;
  return out;
}

/** The rack slots not already used by a pending tile. */
function freeSlots() {
  const used = pending.map((p) => p.from);
  return String((state && state.rack) || '')
    .split('')
    .map((letter, index) => ({ letter, index }))
    .filter((slot) => !used.includes(slot.index));
}

function squareName(at) {
  return ALPHABET[at % SIZE] + String(Math.floor(at / SIZE) + 1);
}

function drawBoard() {
  if (!state) return;
  const grid = cells();
  const premiums = state.premiums || [];
  const fresh = (state.last && state.last.kind === 'word' && state.last.squares) || [];
  const mine = pending.map((p) => p.at);
  board.textContent = '';
  for (let at = 0; at < SIZE * SIZE; at++) {
    const cell = document.createElement('button');
    cell.type = 'button';
    cell.dataset.at = String(at);
    cell.setAttribute('aria-label', squareName(at));
    const tile = grid[at];
    let klass = 'sq';
    if (tile && tile !== '.') {
      klass += ' has';
      if (mine.includes(at)) klass += ' mine';
      else if (fresh.includes(at)) klass += ' fresh';
      const letter = document.createElement('span');
      letter.className = 'l';
      letter.textContent = tile.toUpperCase();
      cell.appendChild(letter);
      // A blank is written lower case and is worth nothing, so it shows no
      // number — which is how you tell one from the letter it stands for.
      if (tile === tile.toUpperCase() && VALUES[tile]) {
        const worth = document.createElement('span');
        worth.className = 'v';
        worth.textContent = String(VALUES[tile]);
        cell.appendChild(worth);
      }
    } else {
      const kind = premiums[at] || '';
      if (kind) klass += ' ' + kind;
      cell.textContent = kind === 'centre' ? '★' : kind ? kind.toUpperCase() : '';
      if (picked !== null && state.your_turn) klass += ' target';
    }
    cell.className = klass;
    board.appendChild(cell);
  }
}

function drawRack() {
  rackBox.textContent = '';
  if (!state) return;
  for (const slot of freeSlots()) {
    const tile = document.createElement('button');
    tile.type = 'button';
    tile.dataset.slot = String(slot.index);
    let klass = 't';
    if (picked === slot.index) klass += ' picked';
    if (swapping.includes(slot.index)) klass += ' swapping';
    if (slot.letter === '?') klass += ' blank';
    tile.className = klass;
    tile.textContent = slot.letter === '?' ? '?' : slot.letter;
    if (VALUES[slot.letter]) {
      const worth = document.createElement('span');
      worth.className = 'v';
      worth.textContent = String(VALUES[slot.letter]);
      tile.appendChild(worth);
    }
    rackBox.appendChild(tile);
  }
  if (!rackBox.children.length) {
    const empty = document.createElement('span');
    empty.className = 'bag';
    empty.textContent = pending.length ? 'Every tile is on the board.' : 'No tiles left.';
    rackBox.appendChild(empty);
  }
}

function drawSeats() {
  const box = el('seats');
  box.textContent = '';
  for (const seat of (state && state.seats) || []) {
    const row = document.createElement('div');
    row.className = 'seat' + (seat.to_play ? ' turn' : '') + (seat.dropped ? ' gone' : '');
    const who = document.createElement('b');
    who.textContent = (seat.crest || '') + ' ' + (seat.you ? 'You' : seat.name);
    const score = document.createElement('span');
    score.className = 'n';
    score.textContent = String(seat.score);
    row.appendChild(who);
    row.appendChild(score);
    const tiles = document.createElement('span');
    tiles.className = 'bag';
    tiles.textContent = seat.dropped ? 'left' : seat.tiles + ' tiles';
    row.appendChild(tiles);
    box.appendChild(row);
  }
}

function render() {
  if (!state) return;
  el('title').textContent = 'Game #' + state.game;
  el('bag').textContent = state.bag + ' in the bag';
  el('clock').textContent = state.your_turn ? state.clock + ' left' : 'waiting';
  el('clock').className = 'clock' + (state.your_turn && state.seconds_left <= 20 ? ' low' : '');
  drawSeats();
  drawBoard();
  drawRack();
  buttons();
}

function buttons() {
  const mine = !!(state && state.your_turn);
  el('play').disabled = !mine || !pending.length || busy;
  el('pass').disabled = !mine || busy;
  el('swap').disabled = !mine || busy || !(state && state.can_exchange);
  el('leave').disabled = busy;
  el('swap').textContent = swapMode ? 'Swap ' + swapping.length : 'Exchange';
}

// --- talking to the server ----------------------------------------------------

async function ask(path, options) {
  const res = await fetch(BASE + path, options);
  if (res.status === 404) {
    gone();
    return null;
  }
  if (!res.ok) throw new Error('server said ' + res.status);
  return res.json();
}

function gone() {
  state = null;
  pending = [];
  picked = null;
  swapping = [];
  swapMode = false;
  if (timer) window.clearInterval(timer);
  timer = null;
  board.textContent = '';
  rackBox.textContent = '';
  picker.hidden = true;
  for (const id of ['play', 'pass', 'swap', 'leave']) el(id).disabled = true;
  say('This board link isn’t open any more. Links stop working when the game ends — the result is in Discord.', 'bad');
}

async function refresh(quiet) {
  try {
    const fresh = await ask('/state');
    if (!fresh) return;
    // Tiles put down this turn are dropped when the turn has moved on, so the
    // board never shows a play that can no longer be made.
    if (state && fresh.turns !== state.turns) {
      pending = [];
      picked = null;
      swapping = [];
      swapMode = false;
    }
    state = fresh;
    render();
    if (!quiet) say(whoseTurn());
  } catch (err) {
    if (!quiet) say('Couldn’t reach the server. It will try again.', 'bad');
  }
}

function whoseTurn() {
  if (!state) return '';
  if (state.dropped) return 'You’ve left this game.';
  if (state.your_turn) return 'Your turn. Tap a tile, then a square.';
  const waiting = (state.seats || []).find((s) => s.to_play);
  return 'Waiting for ' + (waiting ? waiting.name : 'the next player') + '.';
}

/** The tiles as the server wants them: a lower-case letter means a blank. */
function body() {
  return { tiles: pending.map((p) => ({ at: p.at, letter: p.blank ? p.letter.toLowerCase() : p.letter })) };
}

/** Asks the server what the play on the board would score, and says so. */
async function preview() {
  if (!pending.length) {
    say(whoseTurn());
    return;
  }
  const mine = ++previewing;
  try {
    const answer = await ask('/preview', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body()),
    });
    if (!answer || mine !== previewing) return;
    if (answer.ok) {
      const words = (answer.words || []).map((w) => w.word + ' ' + w.score).join(' · ');
      const bonus = answer.bingo ? ' — all seven tiles, fifty on top!' : '';
      note.className = 'note good';
      note.textContent = '';
      const score = document.createElement('span');
      score.className = 'score';
      score.textContent = answer.score + ' points';
      note.appendChild(score);
      note.appendChild(document.createTextNode(' · ' + words + bonus));
    } else {
      say(answer.error || 'That play can’t be made.', 'bad');
    }
  } catch (err) {
    if (mine === previewing) say('Couldn’t work that out just now.', 'bad');
  }
}

async function send(path, payload, confirmation) {
  if (busy) return;
  if (confirmation && !window.confirm(confirmation)) return;
  busy = true;
  buttons();
  try {
    const answer = await ask(path, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload || {}),
    });
    if (!answer) return;
    if (!answer.ok) {
      say(answer.error || 'That was refused.', 'bad');
      return;
    }
    pending = [];
    picked = null;
    swapping = [];
    swapMode = false;
    say(answer.message || '', 'good');
    if (answer.over) {
      window.setTimeout(gone, 2000);
    } else if (answer.state) {
      state = answer.state;
      render();
    }
  } catch (err) {
    say('That didn’t go through. Try again.', 'bad');
  } finally {
    busy = false;
    buttons();
  }
}

// --- tapping ------------------------------------------------------------------

function pickUp(at) {
  const index = pending.findIndex((p) => p.at === at);
  if (index < 0) return false;
  pending.splice(index, 1);
  picked = null;
  render();
  preview();
  return true;
}

board.addEventListener('click', (event) => {
  const cell = event.target.closest('.sq');
  if (!cell || !state) return;
  const at = Number(cell.dataset.at);
  // A tile put down this turn is picked back up by tapping it.
  if (pickUp(at)) return;
  if (!state.your_turn) {
    say(whoseTurn());
    return;
  }
  if (cells()[at] !== '.') {
    say('There’s already a tile on ' + squareName(at) + '.', 'bad');
    return;
  }
  if (picked === null) {
    say('Tap one of your tiles first, then the square it goes on.');
    return;
  }
  const letter = String(state.rack)[picked];
  if (letter === '?') {
    asking = { at: at, from: picked };
    showPicker();
    return;
  }
  pending.push({ at: at, letter: letter, blank: false, from: picked });
  picked = null;
  render();
  preview();
});

rackBox.addEventListener('click', (event) => {
  const tile = event.target.closest('.t');
  if (!tile || !state) return;
  const slot = Number(tile.dataset.slot);
  // While an exchange is being chosen, tapping picks tiles to swap instead.
  if (swapMode) {
    toggleSwap(slot);
    return;
  }
  picked = picked === slot ? null : slot;
  render();
});

function toggleSwap(slot) {
  const index = swapping.indexOf(slot);
  if (index < 0) swapping.push(slot);
  else swapping.splice(index, 1);
  picked = null;
  render();
  say(swapping.length ? 'Press Swap to put those back and draw fresh ones.' : 'Tap the tiles you want to put back.');
}

function showPicker() {
  const box = el('letters');
  box.textContent = '';
  for (const letter of ALPHABET) {
    const button = document.createElement('button');
    button.type = 'button';
    button.dataset.letter = letter;
    button.textContent = letter;
    box.appendChild(button);
  }
  const cancel = document.createElement('button');
  cancel.type = 'button';
  cancel.dataset.letter = '';
  cancel.style.width = 'auto';
  cancel.style.padding = '0 10px';
  cancel.textContent = 'Cancel';
  box.appendChild(cancel);
  picker.hidden = false;
}

picker.addEventListener('click', (event) => {
  const button = event.target.closest('button');
  if (!button || !asking) return;
  const letter = button.dataset.letter;
  const where = asking;
  asking = null;
  picker.hidden = true;
  if (!letter) {
    say('Cancelled.');
    return;
  }
  pending.push({ at: where.at, letter: letter, blank: true, from: where.from });
  picked = null;
  render();
  preview();
});

el('play').addEventListener('click', () => send('/play', body()));
el('pass').addEventListener('click', () => send('/pass', {}, 'Pass this turn?'));
el('leave').addEventListener('click', () => send('/leave', {}, 'Leave the game? Your tiles go back in the bag and you score nothing more.'));
el('swap').addEventListener('click', () => {
  // The first press turns the rack into a chooser; the second, once something
  // is chosen, actually swaps. Pressing it with nothing chosen turns the
  // chooser off again, so there is always a way back out.
  if (!swapMode) {
    swapMode = true;
    swapping = [];
    picked = null;
    pending = [];
    render();
    say('Tap the tiles you want to put back, then press Swap again.');
    return;
  }
  if (!swapping.length) {
    swapMode = false;
    render();
    say(whoseTurn());
    return;
  }
  const tiles = swapping.map((slot) => String(state.rack)[slot]).join('');
  send('/exchange', { tiles: tiles }, 'Put ' + swapping.length + ' tiles back and draw fresh ones?');
});

refresh(false);
timer = window.setInterval(() => refresh(true), 4000);
