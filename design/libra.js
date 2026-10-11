/* Libra — shared shell behaviour: theme, sidebar, citation↔rail linking */

(function () {
  'use strict';

  var THEME_KEY = 'libra:theme';
  var SIDEBAR_KEY = 'libra:sidebar';

  /* ── theme ─────────────────────────────────────────────── */

  function applyTheme(mode) {
    var resolved = mode;
    if (mode === 'system') {
      resolved = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    }
    document.documentElement.dataset.theme = resolved;
    document.documentElement.dataset.themePref = mode;
    document.querySelectorAll('[data-theme-label]').forEach(function (el) {
      el.textContent = resolved === 'dark' ? 'Dark' : 'Light';
    });
    document.querySelectorAll('[data-theme-icon]').forEach(function (el) {
      el.hidden = el.dataset.themeIcon !== resolved;
    });
  }

  function readTheme() {
    try { return localStorage.getItem(THEME_KEY) || 'light'; } catch (e) { return 'light'; }
  }

  function writeTheme(mode) {
    try { localStorage.setItem(THEME_KEY, mode); } catch (e) { /* private mode */ }
    applyTheme(mode);
  }

  window.LibraTheme = { set: writeTheme, get: readTheme, apply: applyTheme };

  /* ── sidebar ───────────────────────────────────────────── */

  function setSidebar(state) {
    var app = document.querySelector('.app');
    if (!app) return;
    app.dataset.sidebar = state;
    try { localStorage.setItem(SIDEBAR_KEY, state); } catch (e) { /* ignore */ }
    document.querySelectorAll('[data-sidebar-toggle]').forEach(function (btn) {
      btn.setAttribute('aria-expanded', String(state !== 'collapsed'));
      btn.title = state === 'collapsed' ? 'Show threads  [' : 'Hide threads  [';
    });
  }

  /* ── citation ↔ rail ───────────────────────────────────── */

  function railFor(ref) {
    return document.querySelector('.source[data-ref="' + ref + '"]');
  }

  function light(ref, on) {
    var card = railFor(ref);
    if (card) card.classList.toggle('is-lit', on);
    document.querySelectorAll('.cite[data-ref="' + ref + '"]').forEach(function (c) {
      c.classList.toggle('is-lit', on);
    });
  }

  /* scroll the rail by hand — scrollIntoView breaks the embedded preview */
  function revealInRail(ref) {
    var card = railFor(ref);
    var scroller = document.querySelector('.rail__scroll');
    if (!card || !scroller) return;
    var top = card.offsetTop - scroller.clientHeight / 2 + card.offsetHeight / 2;
    var max = scroller.scrollHeight - scroller.clientHeight;
    scroller.scrollTo({ top: Math.max(0, Math.min(top, max)), behavior: 'smooth' });
  }

  function bindCitations(root) {
    (root || document).querySelectorAll('.cite').forEach(function (cite) {
      if (cite.dataset.bound) return;
      cite.dataset.bound = '1';
      var ref = cite.dataset.ref;
      cite.addEventListener('mouseenter', function () { light(ref, true); });
      cite.addEventListener('mouseleave', function () { light(ref, false); });
      cite.addEventListener('focus', function () { light(ref, true); });
      cite.addEventListener('blur', function () { light(ref, false); });
      cite.addEventListener('click', function (e) {
        e.preventDefault();
        revealInRail(ref);
        light(ref, true);
        window.setTimeout(function () { light(ref, false); }, 1600);
      });
    });
  }

  window.LibraCitations = { bind: bindCitations, reveal: revealInRail };

  /* ── rail filters ──────────────────────────────────────── */

  function bindRailFilters() {
    var group = document.querySelector('[data-rail-filters]');
    if (!group) return;
    var count = document.querySelector('[data-rail-count]');
    group.addEventListener('click', function (e) {
      var chip = e.target.closest('.chip');
      if (!chip) return;
      group.querySelectorAll('.chip').forEach(function (c) {
        c.setAttribute('aria-pressed', String(c === chip));
      });
      var kind = chip.dataset.kind;
      var shown = 0;
      document.querySelectorAll('.source').forEach(function (card) {
        var match = kind === 'all' || card.dataset.kind === kind;
        card.hidden = !match;
        if (match) shown++;
      });
      if (count) count.textContent = shown + (shown === 1 ? ' source' : ' sources');
    });
  }

  /* ── composer ──────────────────────────────────────────── */

  function bindComposer() {
    document.querySelectorAll('.composer textarea').forEach(function (ta) {
      function grow() {
        ta.style.height = 'auto';
        ta.style.height = Math.min(ta.scrollHeight, 140) + 'px';
      }
      ta.addEventListener('input', grow);
      grow();
    });
  }

  /* ── boot ──────────────────────────────────────────────── */

  document.addEventListener('DOMContentLoaded', function () {
    applyTheme(readTheme());

    var saved = 'expanded';
    try { saved = localStorage.getItem(SIDEBAR_KEY) || 'expanded'; } catch (e) { /* ignore */ }
    if (document.querySelector('.app')) setSidebar(saved);

    document.querySelectorAll('[data-theme-toggle]').forEach(function (btn) {
      btn.addEventListener('click', function () {
        writeTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark');
      });
    });

    document.querySelectorAll('[data-sidebar-toggle]').forEach(function (btn) {
      btn.addEventListener('click', function () {
        var app = document.querySelector('.app');
        setSidebar(app.dataset.sidebar === 'collapsed' ? 'expanded' : 'collapsed');
      });
    });

    bindCitations();
    bindRailFilters();
    bindComposer();

    document.addEventListener('keydown', function (e) {
      var typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName);
      if (e.key === '/' && !typing) {
        var box = document.querySelector('.composer textarea, [data-ask-input]');
        if (box) { e.preventDefault(); box.focus(); }
      }
      if (e.key === '[' && !typing && document.querySelector('.app')) {
        var app = document.querySelector('.app');
        setSidebar(app.dataset.sidebar === 'collapsed' ? 'expanded' : 'collapsed');
      }
    });
  });
})();
