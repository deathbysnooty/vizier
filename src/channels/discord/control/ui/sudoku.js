/* The sudoku page.
 *
 * It knows the puzzle's givens and nothing else: the answer never leaves the
 * bot. All this does is let someone fill the grid in comfortably and then turn
 * what they filled in into a short code for Discord.
 *
 * `sudokuCode` is the twin of `sudoku_code.rs` in the bot. The two must always
 * produce the same string; the shared example is
 *
 *   puzzle 128, givens
 *   530070000600195000098000060800060003400803001700020006060000280000419005000080079
 *   filled in with its answer
 *   -> S128-E3GCU-QQL37-JW254-ZBHZZ-53GSX-D3UPV-Z5CPA-AAIBG-LI
 *
 * which is `sudoku_code::tests::TEST_VECTOR` on the Rust side. Change one and
 * you must change the other.
 */
(function () {
  'use strict';

  var ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  var CELLS = 81;
  var PER_GROUP = 5;
  var GROUP_BITS = 17;

  // --- the code ---------------------------------------------------------------

  function crc16(bytes) {
    var crc = 0xffff;
    for (var i = 0; i < bytes.length; i++) {
      crc ^= (bytes[i] << 8) & 0xffff;
      for (var b = 0; b < 8; b++) {
        crc = crc & 0x8000 ? ((crc << 1) ^ 0x1021) & 0xffff : (crc << 1) & 0xffff;
      }
    }
    return crc;
  }

  function base32(bytes) {
    var out = '';
    var buffer = 0;
    var bits = 0;
    for (var i = 0; i < bytes.length; i++) {
      buffer = (buffer << 8) | bytes[i];
      bits += 8;
      while (bits >= 5) {
        bits -= 5;
        out += ALPHABET[(buffer >> bits) & 31];
      }
    }
    if (bits > 0) out += ALPHABET[(buffer << (5 - bits)) & 31];
    return out;
  }

  /* The code for a grid: only the squares the puzzle left blank, five at a time
     packed into 17 bits, then a CRC-16 of the puzzle number and those bytes. */
  function sudokuCode(id, givens, filled) {
    var cells = [];
    for (var i = 0; i < CELLS; i++) if (givens[i] === 0) cells.push(i);
    var bytes = [];
    var used = 8;
    function push(value, count) {
      for (var bit = count - 1; bit >= 0; bit--) {
        if (used === 8) {
          bytes.push(0);
          used = 0;
        }
        bytes[bytes.length - 1] |= ((value >> bit) & 1) << (7 - used);
        used++;
      }
    }
    for (var at = 0; at < cells.length; at += PER_GROUP) {
      var value = 0;
      var place = 1;
      for (var k = at; k < Math.min(at + PER_GROUP, cells.length); k++) {
        var digit = filled[cells[k]] || 0;
        value += (digit > 9 ? 9 : digit) * place;
        place *= 10;
      }
      push(value, GROUP_BITS);
    }
    var want = Math.ceil((Math.ceil(cells.length / PER_GROUP) * GROUP_BITS) / 8);
    while (bytes.length < want) bytes.push(0);
    var head = [(id >>> 24) & 255, (id >>> 16) & 255, (id >>> 8) & 255, id & 255];
    var crc = crc16(head.concat(bytes));
    bytes.push((crc >> 8) & 255, crc & 255);
    var body = base32(bytes);
    var groups = [];
    for (var g = 0; g < body.length; g += 5) groups.push(body.slice(g, g + 5));
    return 'S' + id + '-' + groups.join('-');
  }

  // The bot's own tests can reach this, and so can anyone checking by hand.
  window.sudokuCode = sudokuCode;

  // --- the puzzle -------------------------------------------------------------

  var root = document.getElementById('game');
  if (!root) return;
  var id = parseInt(root.getAttribute('data-id'), 10) || 0;
  var givenText = root.getAttribute('data-givens') || '';
  if (givenText.length !== CELLS) return;
  var givens = [];
  for (var i = 0; i < CELLS; i++) givens.push(givenText.charCodeAt(i) - 48);
  var posted = parseInt(root.getAttribute('data-posted'), 10) || 0;

  var board = document.getElementById('board');
  var pad = document.getElementById('pad');
  var statusLine = document.getElementById('status');
  var codeBox = document.getElementById('code');
  var copyButton = document.getElementById('copy');
  var notesButton = document.getElementById('notes');
  var timer = document.getElementById('timer');

  var filled = givens.slice();
  var notes = [];
  for (i = 0; i < CELLS; i++) notes.push(0);
  var history = [];
  var selected = givens.indexOf(0);
  var notesMode = false;
  var checking = false;
  var cells = [];

  var STORE = 'sudoku:' + id;

  function save() {
    try {
      window.localStorage.setItem(STORE, JSON.stringify({ filled: filled, notes: notes }));
    } catch (e) {
      /* a private window, or storage turned off: the grid still works */
    }
  }

  function load() {
    try {
      var raw = window.localStorage.getItem(STORE);
      if (!raw) return;
      var saved = JSON.parse(raw);
      if (!saved || !saved.filled || saved.filled.length !== CELLS) return;
      for (var c = 0; c < CELLS; c++) {
        if (givens[c] !== 0) continue;
        var digit = saved.filled[c];
        if (typeof digit === 'number' && digit >= 0 && digit <= 9) filled[c] = digit;
        if (saved.notes && typeof saved.notes[c] === 'number') notes[c] = saved.notes[c];
      }
    } catch (e) {
      /* unreadable: start fresh */
    }
  }

  function boxOf(cell) {
    return Math.floor(cell / 27) * 3 + Math.floor((cell % 9) / 3);
  }

  /* Squares that clash: the same digit twice in a row, a column or a box. The
     answer is never involved — this only checks the rules. */
  function clashes() {
    var bad = {};
    for (var a = 0; a < CELLS; a++) {
      if (filled[a] === 0) continue;
      for (var b = a + 1; b < CELLS; b++) {
        if (filled[b] !== filled[a]) continue;
        var sameRow = Math.floor(a / 9) === Math.floor(b / 9);
        var sameCol = a % 9 === b % 9;
        if (sameRow || sameCol || boxOf(a) === boxOf(b)) {
          bad[a] = true;
          bad[b] = true;
        }
      }
    }
    return bad;
  }

  function left() {
    var n = 0;
    for (var c = 0; c < CELLS; c++) if (filled[c] === 0) n++;
    return n;
  }

  function draw() {
    var bad = checking ? clashes() : {};
    var chosen = selected >= 0 ? filled[selected] : 0;
    for (var c = 0; c < CELLS; c++) {
      var cell = cells[c];
      var classes = 'cell';
      if (givens[c] !== 0) classes += ' given';
      if (Math.floor(c / 9) % 3 === 2 && c < 72) classes += ' r3';
      if (selected >= 0) {
        var peer =
          Math.floor(c / 9) === Math.floor(selected / 9) || c % 9 === selected % 9 || boxOf(c) === boxOf(selected);
        if (peer && c !== selected) classes += ' peer';
        if (chosen !== 0 && filled[c] === chosen && c !== selected) classes += ' same';
        if (c === selected) classes += ' sel';
      }
      if (bad[c]) classes += ' bad';
      cell.className = classes;
      cell.textContent = '';
      if (filled[c] !== 0) {
        cell.textContent = String(filled[c]);
      } else if (notes[c]) {
        var marks = document.createElement('span');
        marks.className = 'notes';
        for (var d = 1; d <= 9; d++) {
          var slot = document.createElement('span');
          slot.textContent = notes[c] & (1 << (d - 1)) ? String(d) : '';
          marks.appendChild(slot);
        }
        cell.appendChild(marks);
      }
      cell.setAttribute('aria-label', squareName(c) + (filled[c] ? ' is ' + filled[c] : ' empty'));
    }
    drawPad();
    drawStatus(bad);
    drawCode();
  }

  function squareName(cell) {
    return String.fromCharCode(97 + Math.floor(cell / 9)) + ((cell % 9) + 1);
  }

  function drawPad() {
    var counts = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    for (var c = 0; c < CELLS; c++) counts[filled[c]]++;
    for (var d = 1; d <= 9; d++) {
      var key = pad.children[d - 1];
      key.className = 'key' + (counts[d] >= 9 ? ' done' : '');
    }
  }

  function drawStatus(bad) {
    var wrong = Object.keys(bad).length;
    var togo = left();
    statusLine.className = 'status';
    if (wrong > 0) {
      statusLine.className = 'status warn';
      statusLine.textContent =
        '⚠️ ' + wrong + (wrong === 1 ? ' square clashes' : ' squares clash') + ' with another in its row, column or box.';
    } else if (togo === 0) {
      statusLine.textContent = '✅ Every square is filled — press Copy code, then Submit code in Discord.';
    } else if (checking) {
      statusLine.textContent = '✅ No clashes so far · ' + togo + ' squares to go · your work is saved on this device';
    } else {
      statusLine.className = 'status plain';
      statusLine.textContent = togo + ' squares to go · your work is saved on this device';
    }
  }

  function drawCode() {
    var togo = left();
    codeBox.value = sudokuCode(id, givens, filled);
    copyButton.disabled = false;
    codeBox.setAttribute('aria-label', togo === 0 ? 'Your finished code' : 'Your code so far');
  }

  function put(cell, digit) {
    if (givens[cell] !== 0) return;
    history.push({ cell: cell, was: filled[cell], notes: notes[cell] });
    if (history.length > 200) history.shift();
    if (notesMode && digit !== 0) {
      if (filled[cell] === 0) notes[cell] ^= 1 << (digit - 1);
    } else {
      filled[cell] = filled[cell] === digit ? 0 : digit;
      if (filled[cell] !== 0) notes[cell] = 0;
    }
    save();
    draw();
  }

  function undo() {
    var step = history.pop();
    if (!step) return;
    filled[step.cell] = step.was;
    notes[step.cell] = step.notes;
    selected = step.cell;
    save();
    draw();
  }

  function reset() {
    for (var c = 0; c < CELLS; c++) {
      filled[c] = givens[c];
      notes[c] = 0;
    }
    history = [];
    checking = false;
    save();
    draw();
  }

  function select(cell) {
    selected = cell;
    draw();
  }

  // --- building the page ------------------------------------------------------

  for (i = 0; i < CELLS; i++) {
    var cell = document.createElement('button');
    cell.type = 'button';
    cell.className = 'cell';
    cell.setAttribute('data-cell', String(i));
    cells.push(cell);
    board.appendChild(cell);
  }

  board.addEventListener('click', function (event) {
    var target = event.target.closest('.cell');
    if (!target) return;
    select(parseInt(target.getAttribute('data-cell'), 10));
  });

  for (i = 1; i <= 9; i++) {
    var key = document.createElement('button');
    key.type = 'button';
    key.className = 'key';
    key.textContent = String(i);
    key.setAttribute('data-digit', String(i));
    pad.appendChild(key);
  }
  var clear = document.createElement('button');
  clear.type = 'button';
  clear.className = 'key clear';
  clear.textContent = 'Clear';
  clear.setAttribute('data-digit', '0');
  pad.appendChild(clear);

  pad.addEventListener('click', function (event) {
    var target = event.target.closest('[data-digit]');
    if (!target || selected < 0) return;
    put(selected, parseInt(target.getAttribute('data-digit'), 10));
  });

  notesButton.addEventListener('click', function () {
    notesMode = !notesMode;
    notesButton.setAttribute('aria-pressed', notesMode ? 'true' : 'false');
  });

  document.getElementById('undo').addEventListener('click', undo);

  document.getElementById('check').addEventListener('click', function () {
    checking = true;
    draw();
  });

  document.getElementById('reset').addEventListener('click', function () {
    if (left() === CELLS - countGivens() || window.confirm('Clear everything you have filled in?')) reset();
  });

  function countGivens() {
    var n = 0;
    for (var c = 0; c < CELLS; c++) if (givens[c] !== 0) n++;
    return n;
  }

  copyButton.addEventListener('click', function () {
    var text = codeBox.value;
    var done = function () {
      copyButton.textContent = 'Copied ✓';
      window.setTimeout(function () {
        copyButton.textContent = 'Copy code';
      }, 1600);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
    function fallback() {
      codeBox.removeAttribute('readonly');
      codeBox.focus();
      codeBox.setSelectionRange(0, text.length);
      var copied = false;
      try {
        copied = document.execCommand('copy');
      } catch (e) {
        copied = false;
      }
      codeBox.setAttribute('readonly', 'readonly');
      if (copied) done();
      else {
        copyButton.textContent = 'Copy it ↑';
        window.setTimeout(function () {
          copyButton.textContent = 'Copy code';
        }, 2400);
      }
    }
  });

  document.addEventListener('keydown', function (event) {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    var key = event.key;
    if (key >= '1' && key <= '9') {
      if (selected >= 0) put(selected, parseInt(key, 10));
      event.preventDefault();
      return;
    }
    if (key === '0' || key === 'Backspace' || key === 'Delete') {
      if (selected >= 0) put(selected, 0);
      event.preventDefault();
      return;
    }
    var moves = { ArrowUp: -9, ArrowDown: 9, ArrowLeft: -1, ArrowRight: 1 };
    if (moves[key] !== undefined) {
      var next = selected + moves[key];
      if (key === 'ArrowLeft' && selected % 9 === 0) next = selected;
      if (key === 'ArrowRight' && selected % 9 === 8) next = selected;
      if (next >= 0 && next < CELLS) select(next);
      event.preventDefault();
      return;
    }
    if (key === 'n' || key === 'N') notesButton.click();
  });

  function tick() {
    if (!posted) return;
    var open = Math.max(0, Math.floor(Date.now() / 1000) - posted);
    var hours = Math.floor(open / 3600);
    var mins = Math.floor((open % 3600) / 60);
    var secs = open % 60;
    var pad2 = function (n) {
      return n < 10 ? '0' + n : String(n);
    };
    timer.textContent = '⏱️ ' + (hours > 0 ? hours + ':' + pad2(mins) + ':' + pad2(secs) : mins + ':' + pad2(secs));
  }

  load();
  draw();
  tick();
  window.setInterval(tick, 1000);
})();
