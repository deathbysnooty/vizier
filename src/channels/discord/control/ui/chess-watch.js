// The watching page, and the replay a game becomes when it is over.
//
// It reads a game's number out of the address bar and asks the server for the
// position, the clocks and every board the game has passed through. There is
// nothing to press while a game is running: it just follows along. Once the
// game has ended the step buttons appear and walk through those same boards, so
// the replay needs no chess knowledge here at all.
//
// Nothing on this page belongs to a player. There is no link to move with and
// no way to ask for one.

'use strict';

const parts = window.location.pathname.split('/').filter(Boolean);
const GAME = parts[2] || '';
const BASE = '/chess/watch/' + encodeURIComponent(GAME);

const FILES = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
const GLYPH = { k: '♚', q: '♛', r: '♜', b: '♝', n: '♞', p: '♟' };

const el = (id) => document.getElementById(id);
const board = el('board');
const note = el('note');
const steps = el('steps');

let state = null;
let at = 0;          // which board is being shown
let following = true; // true while the newest board is the one on screen
let timer = null;

function say(text, kind) {
  note.textContent = text || '';
  note.className = 'note' + (kind ? ' ' + kind : '');
}

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
      out[FILES[file] + String(8 - r)] = { role: c.toLowerCase(), white: c === c.toUpperCase() };
      file++;
    }
  }
  return out;
}

function coordinate(kind, text) {
  const tag = document.createElement('span');
  tag.className = 'co ' + kind;
  tag.textContent = text;
  return tag;
}

/** Always drawn from White's side: a watcher has no side of their own. */
function draw(frame) {
  const pieces = placement(frame.fen);
  board.textContent = '';
  for (let row = 0; row < 8; row++) {
    for (let file = 0; file < 8; file++) {
      const rank = 8 - row;
      const square = FILES[file] + String(rank);
      const cell = document.createElement('div');
      cell.className = 'sq' + ((file + rank) % 2 === 0 ? ' lightsq' : '');
      cell.dataset.square = square;
      if (frame.last && (frame.last.slice(0, 2) === square || frame.last.slice(2, 4) === square)) {
        cell.classList.add('moved');
      }
      if (frame.check === square) cell.classList.add('check');
      const piece = pieces[square];
      if (piece) {
        const span = document.createElement('span');
        span.className = 'p ' + (piece.white ? 'w' : 'b');
        span.textContent = GLYPH[piece.role] || '';
        cell.appendChild(span);
      }
      if (rank === 1) cell.appendChild(coordinate('f', FILES[file]));
      if (file === 0) cell.appendChild(coordinate('r', String(rank)));
      board.appendChild(cell);
    }
  }
}

function moveList(moves, upto) {
  if (!moves || !moves.length) return 'No moves yet.';
  const out = [];
  for (let i = 0; i < moves.length; i += 2) {
    const white = mark(moves[i], i, upto);
    const black = moves[i + 1] ? ' ' + mark(moves[i + 1], i + 1, upto) : '';
    out.push(i / 2 + 1 + '. ' + white + black);
  }
  return out.join('   ');
}

/** The move the board is showing is picked out of the list. */
function mark(san, index, upto) {
  return index === upto - 1 ? '' + san + '' : san;
}

function writeMoves(text) {
  const moves = el('moves');
  moves.textContent = '';
  for (const piece of text.split('')) {
    const [hit, rest] = piece.split('');
    if (rest === undefined) {
      moves.appendChild(document.createTextNode(piece));
      continue;
    }
    const span = document.createElement('span');
    span.className = 'now';
    span.textContent = hit;
    moves.appendChild(span);
    moves.appendChild(document.createTextNode(rest));
  }
}

function seat(which) {
  const side = state[which];
  el(which + '-name').textContent = side.name;
  el(which + '-crest').textContent = side.crest || '';
  el(which + '-clock').textContent = side.clock ? '· ' + side.clock : '';
  const row = el('seat-' + which);
  row.classList.toggle('turn', !!side.to_move);
  row.classList.toggle('won', state.winner === which);
}

function render() {
  if (!state) return;
  const frames = state.frames || [];
  if (following) at = frames.length - 1;
  at = Math.max(0, Math.min(at, frames.length - 1));
  el('title').textContent = (state.running ? 'Game #' : 'Replay of game #') + state.game;
  el('pace').textContent = state.pace + (state.same_house ? ' · no house points' : '');
  seat('white');
  seat('black');
  draw(frames[at] || { fen: '', last: '', check: '' });
  writeMoves(moveList(state.moves, at));
  el('steps').hidden = state.running || frames.length < 2;
  el('at').textContent = at === 0 ? 'before the first move' : 'after ' + frames[at].san;
  el('first').disabled = at === 0;
  el('back').disabled = at === 0;
  el('next').disabled = at >= frames.length - 1;
  el('last').disabled = at >= frames.length - 1;
  renderResult();
  if (state.running) {
    const turn = state[state.turn];
    say('Move ' + state.move_number + ' · ' + turn.name + ' to play.');
  } else {
    say('');
  }
}

function renderResult() {
  const box = el('result');
  if (state.running) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  box.textContent = '';
  const head = document.createElement('b');
  head.textContent = state.headline || 'Game over';
  box.appendChild(head);
  const who = state.winner ? state[state.winner].name + ' won' : 'It was a draw';
  const points = ['white', 'black']
    .filter((s) => state[s].points > 0)
    .map((s) => state[s].name + ' +' + state[s].points);
  const tail = points.length ? ' · house points: ' + points.join(', ') : ' · no house points' + (state.why_nothing ? ': ' + state.why_nothing : '');
  box.appendChild(document.createTextNode('\n' + who + ' after ' + state.lasted + '.' + tail));
  box.style.whiteSpace = 'pre-wrap';
}

function gone(message) {
  state = null;
  if (timer) clearInterval(timer);
  timer = null;
  board.textContent = '';
  steps.hidden = true;
  el('result').hidden = true;
  el('moves').textContent = '';
  say(message, 'bad');
}

async function refresh() {
  try {
    const res = await fetch(BASE + '/state');
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      gone(body.error || 'This game isn’t here.');
      return;
    }
    state = body;
    render();
    if (!state.running && timer) {
      // A finished game never changes again: stop asking.
      clearInterval(timer);
      timer = null;
    }
  } catch (err) {
    say('Couldn’t reach the server. It will try again.', 'bad');
  }
}

function step(to) {
  if (!state) return;
  following = false;
  at = to;
  render();
}

el('first').addEventListener('click', () => step(0));
el('back').addEventListener('click', () => step(at - 1));
el('next').addEventListener('click', () => step(at + 1));
el('last').addEventListener('click', () => step((state.frames || []).length - 1));
document.addEventListener('keydown', (event) => {
  if (steps.hidden) return;
  if (event.key === 'ArrowLeft') step(at - 1);
  if (event.key === 'ArrowRight') step(at + 1);
});

refresh();
timer = window.setInterval(refresh, 5000);
