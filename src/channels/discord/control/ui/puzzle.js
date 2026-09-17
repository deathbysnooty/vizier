// The private chess-puzzle page.
//
// It reads the puzzle and the link out of the address bar, asks the server what
// the position is, draws it from the solver's own side, and sends back the move
// they tap. The server decides whether the move is right, so this file only has
// to be pleasant, not trustworthy — and it is never told the answer: the moves
// it lights up are every LEGAL move, and the line comes back one move at a time
// as it is played. Nothing is kept in the browser.

'use strict';

const parts = window.location.pathname.split('/').filter(Boolean);
const PUZZLE = parts[1] || '';
const TOKEN = parts[2] || '';
const BASE = '/puzzle/' + encodeURIComponent(PUZZLE) + '/' + encodeURIComponent(TOKEN);

const FILES = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
// U+FE0E after each is the "draw this as text, not emoji" mark: without it
// iOS draws the pawn as a black emoji, so a white pawn came out black.
const GLYPH = { k: '♚︎', q: '♛︎', r: '♜︎', b: '♝︎', n: '♞︎', p: '♟︎' };
const BANDS = { easy: '🟢 Easy', medium: '🟡 Medium', hard: '🔴 Hard' };

const el = (id) => document.getElementById(id);
const board = el('board');
const note = el('note');
const promo = el('promo');

let state = null;
let picked = null;      // the square a piece was tapped on
let pending = null;     // a promotion waiting for its piece
let busy = false;

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
  if (!state || state.solved) return [];
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
  el('title').textContent = 'Puzzle #' + state.puzzle;
  el('grade').textContent = (BANDS[state.band] || state.band || '') + (state.rating ? ' · ' + state.rating : '');
  el('setup').textContent = state.setup || '…';
  const mine = state.you === 'white' ? 'White' : 'Black';
  const theirs = state.you === 'white' ? 'Black' : 'White';
  el('them-side').textContent = (state.you === 'white' ? '⬛ ' : '⬜ ') + theirs;
  el('me-side').textContent = state.solved ? 'Solved' : 'Your move — you are ' + mine;
  el('steps').textContent = state.done + ' of ' + state.total;
  el('seat-me').classList.toggle('turn', !state.solved);
  el('moves').textContent = moveList(state.played);
  draw();
}

function moveList(moves) {
  if (!moves || !moves.length) return 'Nothing played yet.';
  const out = [];
  for (let i = 0; i < moves.length; i += 2) {
    out.push(i / 2 + 1 + '. ' + moves[i] + (moves[i + 1] ? ' ' + moves[i + 1] : ''));
  }
  return out.join('   ');
}

function prompt() {
  if (!state) return '';
  if (state.solved) return 'Solved. The card in Discord has the rest.';
  const left = Math.max(0, state.total - state.done);
  if (state.done > 0) return 'Right so far — ' + left + (left === 1 ? ' move' : ' moves') + ' to go.';
  return 'Find the move that wins it. A wrong move is just refused, so try as often as you like.';
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
  board.textContent = '';
  promo.hidden = true;
  say('This puzzle link isn’t open any more. Links stop working when the puzzle is replaced — the answer is on its card in Discord.', 'bad');
}

async function refresh(quiet) {
  try {
    const fresh = await ask('/state');
    if (!fresh) return;
    state = fresh;
    render();
    if (!quiet) say(prompt());
  } catch (err) {
    if (!quiet) say('Couldn’t reach the server. Try again in a moment.', 'bad');
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
    if (answer.state) {
      state = answer.state;
      render();
    }
    if (answer.ok) {
      say(answer.message || prompt(), 'good');
    } else {
      say(answer.message || 'That move was refused.', 'bad');
    }
  } catch (err) {
    say('Couldn’t send that move. Try again.', 'bad');
  } finally {
    busy = false;
  }
}

// --- tapping -----------------------------------------------------------------

board.addEventListener('click', (event) => {
  const cell = event.target.closest('.sq');
  if (!cell || !state) return;
  const square = cell.dataset.square;
  if (state.solved) {
    say(prompt());
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

refresh(false);
