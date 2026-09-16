// The private board page.
//
// It reads the game and the link out of the address bar, asks the server what
// the position is, draws it from the player's own side, and sends back the
// move they tap. The server checks everything again, so this file only has to
// be pleasant, not trustworthy. Nothing is kept in the browser: close the tab
// and no trace of the link is left behind.

'use strict';

const parts = window.location.pathname.split('/').filter(Boolean);
const GAME = parts[1] || '';
const TOKEN = parts[2] || '';
const BASE = '/chess/' + encodeURIComponent(GAME) + '/' + encodeURIComponent(TOKEN);

const FILES = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
// The solid glyphs for both sides; colour and an outline tell them apart.
const GLYPH = { k: '♚', q: '♛', r: '♜', b: '♝', n: '♞', p: '♟' };

const el = (id) => document.getElementById(id);
const board = el('board');
const note = el('note');
const promo = el('promo');

let state = null;
let picked = null;      // the square a piece was tapped on
let pending = null;     // a promotion waiting for its piece
let busy = false;
let timer = null;

function say(text, kind) {
  note.textContent = text || '';
  note.className = 'note' + (kind ? ' ' + kind : '');
}

// --- the position ------------------------------------------------------------

/** The piece placement field of a FEN as a map of square to piece. */
function placement(fen) {
  const out = {};
  const rows = String(fen || '').split(' ')[0].split('/');
  for (let r = 0; r < rows.length && r < 8; r++) {
    let file = 0;
    for (const c of rows[r]) {
      if (c >= '1' && c <= '8') {
        file += Number(c);
        continue;
      }
      if (file > 7) break;
      const square = FILES[file] + String(8 - r);
      out[square] = { role: c.toLowerCase(), white: c === c.toUpperCase() };
      file++;
    }
  }
  return out;
}

/** Every legal move from a square, as told by the server. */
function movesFrom(square) {
  if (!state || !state.your_move) return [];
  return (state.legal || []).filter((m) => m.from === square);
}

function draw() {
  if (!state) return;
  const pieces = placement(state.fen);
  const flipped = state.you === 'black';
  const targets = picked ? movesFrom(picked) : [];
  board.textContent = '';
  for (let row = 0; row < 8; row++) {
    for (let col = 0; col < 8; col++) {
      const file = flipped ? 7 - col : col;
      const rank = flipped ? row + 1 : 8 - row;
      const square = FILES[file] + String(rank);
      const light = (file + rank) % 2 === 0;
      const cell = document.createElement('button');
      cell.type = 'button';
      cell.className = 'sq' + (light ? ' lightsq' : '');
      cell.dataset.square = square;
      cell.setAttribute('aria-label', square);
      if (state.last && (state.last.slice(0, 2) === square || state.last.slice(2, 4) === square)) {
        cell.classList.add('moved');
      }
      if (state.check === square) cell.classList.add('check');
      if (picked === square) cell.classList.add('picked');
      const piece = pieces[square];
      if (piece) {
        const span = document.createElement('span');
        span.className = 'p ' + (piece.white ? 'w' : 'b');
        span.textContent = GLYPH[piece.role] || '';
        cell.appendChild(span);
      }
      if (targets.some((m) => m.to === square)) {
        if (piece) cell.classList.add('takeable');
        const dot = document.createElement('span');
        dot.className = 'dot';
        cell.appendChild(dot);
      }
      const bottom = flipped ? rank === 8 : rank === 1;
      const left = flipped ? file === 7 : file === 0;
      if (bottom) cell.appendChild(coordinate('f', FILES[file]));
      if (left) cell.appendChild(coordinate('r', String(rank)));
      board.appendChild(cell);
    }
  }
}

function coordinate(kind, text) {
  const tag = document.createElement('span');
  tag.className = 'co ' + kind;
  tag.textContent = text;
  return tag;
}

// --- the rest of the page ----------------------------------------------------

function render() {
  if (!state) return;
  el('title').textContent = 'Game #' + state.game;
  el('pace').textContent = state.pace + (state.same_house ? ' · no house points' : '');
  el('them-name').textContent = state.them;
  el('me-name').textContent = state.me;
  el('them-crest').textContent = state.their_crest || '';
  el('me-crest').textContent = state.my_crest || '';
  const mine = state.you === 'white' ? 'White' : 'Black';
  const theirs = state.you === 'white' ? 'Black' : 'White';
  el('me-side').textContent = (state.you === 'white' ? '⬜ ' : '⬛ ') + mine;
  el('them-side').textContent = (state.you === 'white' ? '⬛ ' : '⬜ ') + theirs;
  el('clock').textContent = state.clock;
  el('clock').className = 'clock' + (state.seconds_left <= 120 ? ' low' : '');
  el('seat-me').classList.toggle('turn', !!state.your_move);
  el('seat-them').classList.toggle('turn', !state.your_move);
  el('moves').textContent = moveList(state.moves);
  const drawButton = el('draw');
  if (state.draw_offer === 'them') {
    drawButton.textContent = '\u{1F91D} Accept their draw';
  } else if (state.draw_offer === 'you') {
    drawButton.textContent = '\u{1F91D} Draw offered';
  } else {
    drawButton.textContent = '\u{1F91D} Offer draw';
  }
  drawButton.disabled = state.draw_offer === 'you';
  draw();
}

function moveList(moves) {
  if (!moves || !moves.length) return 'No moves yet.';
  const out = [];
  for (let i = 0; i < moves.length; i += 2) {
    out.push(i / 2 + 1 + '. ' + moves[i] + (moves[i + 1] ? ' ' + moves[i + 1] : ''));
  }
  return out.join('   ');
}

function whoseTurn() {
  if (!state) return '';
  if (state.your_move) return 'Your move.';
  return 'Waiting for ' + state.them + '.';
}

// --- talking to the server ---------------------------------------------------

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
  picked = null;
  if (timer) clearInterval(timer);
  timer = null;
  board.textContent = '';
  promo.hidden = true;
  el('resign').disabled = true;
  el('draw').disabled = true;
  say('This board link isn’t open any more. Links stop working when the game ends — the result is in Discord.', 'bad');
}

async function refresh(quiet) {
  try {
    const fresh = await ask('/state');
    if (!fresh) return;
    state = fresh;
    render();
    if (!quiet) say(whoseTurn());
  } catch (err) {
    if (!quiet) say('Couldn’t reach the server. It will try again.', 'bad');
  }
}

async function send(long) {
  if (busy) return;
  busy = true;
  picked = null;
  promo.hidden = true;
  pending = null;
  try {
    const answer = await ask('/move', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ move: long }),
    });
    if (!answer) return;
    if (answer.ok) {
      if (answer.state) {
        state = answer.state;
        render();
      }
      say(answer.san + ' played. ' + whoseTurn(), 'good');
    } else {
      say(answer.error || 'That move was refused.', 'bad');
      await refresh(true);
    }
  } catch (err) {
    say('Couldn’t send that move. Try again.', 'bad');
  } finally {
    busy = false;
  }
}

async function act(path, confirmation) {
  if (busy) return;
  if (confirmation && !window.confirm(confirmation)) return;
  busy = true;
  try {
    const answer = await ask(path, { method: 'POST' });
    if (!answer) return;
    say(answer.message || '', 'good');
    if (answer.over) {
      window.setTimeout(gone, 1200);
    } else {
      await refresh(true);
    }
  } catch (err) {
    say('That didn’t go through. Try again.', 'bad');
  } finally {
    busy = false;
  }
}

// --- tapping -----------------------------------------------------------------

board.addEventListener('click', (event) => {
  const cell = event.target.closest('.sq');
  if (!cell || !state) return;
  const square = cell.dataset.square;
  if (!state.your_move) {
    say(whoseTurn());
    return;
  }
  if (picked) {
    const options = movesFrom(picked).filter((m) => m.to === square);
    if (options.length > 1) {
      pending = { from: picked, to: square };
      promo.hidden = false;
      say('Which piece?');
      return;
    }
    if (options.length === 1) {
      send(picked + square + (options[0].promotion || ''));
      return;
    }
  }
  picked = movesFrom(square).length ? square : null;
  if (!picked && square) {
    const mine = placement(state.fen)[square];
    if (mine) say('That piece has no legal move.');
  }
  draw();
});

promo.addEventListener('click', (event) => {
  const button = event.target.closest('button');
  if (!button || !pending) return;
  const piece = button.dataset.piece;
  const move = pending;
  pending = null;
  promo.hidden = true;
  picked = null;
  if (piece === 'x') {
    draw();
    say('Cancelled.');
    return;
  }
  send(move.from + move.to + piece);
});

el('resign').addEventListener('click', () => act('/resign', 'Resign this game?'));
el('draw').addEventListener('click', () => {
  const asking = state && state.draw_offer === 'them' ? 'Accept the draw?' : 'Offer a draw?';
  act('/draw', asking);
});

refresh(false);
timer = window.setInterval(() => refresh(true), 5000);
