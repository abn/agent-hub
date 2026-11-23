(() => {
  const top = document.getElementById('top');
  const onScroll = () => top.classList.toggle('scrolled', scrollY > 8);
  addEventListener('scroll', onScroll, { passive: true }); onScroll();

  const COPY_OK = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 8.5l3 3 7-7"/></svg>';
  document.querySelectorAll('[data-copy]').forEach(b => {
    const icon = b.innerHTML, label = b.getAttribute('aria-label');
    b.addEventListener('click', async () => {
      try { await navigator.clipboard.writeText(b.dataset.copy); } catch { return; }
      b.innerHTML = COPY_OK; b.dataset.done = ''; b.setAttribute('aria-label', 'Copied');
      setTimeout(() => { b.innerHTML = icon; delete b.dataset.done; b.setAttribute('aria-label', label); }, 1600);
    });
  });

  // Install steps 1 and 3: one tab per install route and per harness. Without
  // script every panel stays stacked under its heading; with it the headings
  // become the tabs.
  document.querySelectorAll('[data-tabset]').forEach(harness => {
    const panels = [...harness.querySelectorAll('.hpanel')], KEY = harness.dataset.key;
    const list = document.createElement('div'), tabs = [];
    list.className = 'tabs'; list.setAttribute('role', 'tablist'); list.setAttribute('aria-label', harness.dataset.tabset);
    panels.forEach(p => {
      const tab = document.createElement('button');
      tab.type = 'button'; tab.id = 'tab-' + p.id; tab.className = 'tab'; tab.textContent = p.dataset.tab;
      tab.setAttribute('role', 'tab'); tab.setAttribute('aria-controls', p.id);
      p.setAttribute('role', 'tabpanel'); p.setAttribute('aria-labelledby', tab.id);
      tab.addEventListener('click', () => pick(tabs.indexOf(tab)));
      tabs.push(tab); list.append(tab);
    });
    const pick = (i, focus) => {
      tabs.forEach((t, k) => {
        t.setAttribute('aria-selected', k === i); t.tabIndex = k === i ? 0 : -1; panels[k].toggleAttribute('data-off', k !== i);
      });
      // Bring the tab into view inside its own strip; never scroll the page.
      const t = tabs[i];
      const lr = list.getBoundingClientRect(), tr = t.getBoundingClientRect();
      const left = tr.left - lr.left + list.scrollLeft, right = left + tr.width;
      if (left < list.scrollLeft) list.scrollLeft = left;
      else if (right > list.scrollLeft + list.clientWidth) list.scrollLeft = right - list.clientWidth;
      if (focus) t.focus({ preventScroll: true });
      try { localStorage.setItem(KEY, panels[i].id); } catch {}
    };
    list.addEventListener('keydown', e => {
      const at = tabs.indexOf(document.activeElement), n = tabs.length;
      const to = { ArrowRight: (at + 1) % n, ArrowLeft: (at + n - 1) % n, Home: 0, End: n - 1 }[e.key];
      if (at < 0 || to === undefined || e.metaKey || e.ctrlKey || e.altKey) return;
      e.preventDefault(); pick(to, true);
    });
    // The strip scrolls on a narrow phone; an edge fades while more lies beyond it.
    const edges = () => {
      list.toggleAttribute('data-more-start', list.scrollLeft > 1);
      list.toggleAttribute('data-more-end', list.scrollLeft + list.clientWidth < list.scrollWidth - 1);
    };
    list.addEventListener('scroll', edges, { passive: true }); addEventListener('resize', edges);
    harness.prepend(list); harness.classList.add('tabbed');
    const named = () => panels.findIndex(p => p.id === location.hash.slice(1));
    let first = named();
    if (first < 0) { try { first = panels.findIndex(p => p.id === localStorage.getItem(KEY)); } catch {} }
    pick(Math.max(first, 0)); edges();
    // A hidden panel cannot be scrolled to, so a link to one opens it first.
    addEventListener('hashchange', () => { const i = named(); if (i >= 0) { pick(i); harness.scrollIntoView(); } });
    if (named() >= 0) requestAnimationFrame(() => harness.scrollIntoView());
  });

  // The desk: made-up asks, answered with the same keys as the app.
  const asks = [
    { who: 'release-bot', text: 'wants to ship 1.4.2 to the NAS.', meta: 'homelab, approval', verbs: ['Approve', 'Hold'], past: ['approved the 1.4.2 release', 'held the 1.4.2 release'] },
    { who: 'researcher', text: 'asks which of two storage layouts to write up.', meta: 'notes, question', verbs: ['Layout A', 'Layout B'], past: ['chose layout A', 'chose layout B'] },
    { who: 'tidy', text: 'wants to prune 14 finished sessions.', meta: 'agents, approval', verbs: ['Prune', 'Keep'], past: ['pruned 14 sessions', 'kept 14 sessions'] },
  ];
  const later = { who: 'nightly', text: 'found a flaky test and asks whether to quarantine it.', meta: 'ci, question', verbs: ['Quarantine', 'Leave it'], past: ['quarantined the flaky test', 'left the flaky test'] };
  const since = [
    { text: 'docs-writer published "Upgrade notes"', at: '09:12' },
    { text: 'reviewer finished a review of the redesign', at: '08:47' },
  ];
  let waiting = asks.slice(), sel = 0, arrived = false, clock = 9 * 60 + 41;

  const wEl = document.getElementById('waiting'), sEl = document.getElementById('since-list'), cEl = document.getElementById('count');
  const esc = s => s.replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
  const t = () => { const m = clock++; return String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0'); };

  function render(newIdx) {
    cEl.innerHTML = waiting.length ? `<b>${waiting.length}</b> need${waiting.length === 1 ? 's' : ''} you` : 'nothing waiting';
    if (!waiting.length) {
      wEl.innerHTML = '<div class="calm"><b>All clear.</b> Agents keep working; anything new lands here.</div>';
    } else {
      wEl.innerHTML = '<div class="tier-label">Needs you</div>' + waiting.map((a, i) => `
        <div class="ask${i === newIdx ? ' arrive' : ''}" role="listitem" id="ask-${i}" aria-current="${i === sel}" tabindex="${i === sel ? 0 : -1}" data-i="${i}">
          <div><p><b>${esc(a.who)}</b> ${esc(a.text)}</p><div class="meta">${esc(a.meta)}</div></div>
          <div class="acts"><div class="acts-in">${a.verbs.map((v, k) => `<button type="button" class="act${k === 0 ? ' primary' : ''}" data-k="${k}" tabindex="${i === sel ? 0 : -1}"><kbd>${k + 1}</kbd>${esc(v)}</button>`).join('')}</div></div>
        </div>`).join('');
    }
    sEl.innerHTML = since.slice(0, 4).map(s => `<div class="done"><span>${esc(s.text)}</span><time>${s.at}</time></div>`).join('');
  }
  function answer(i, k) {
    const a = waiting[i]; if (!a) return;
    since.unshift({ text: `you ${a.past[k]}`, at: t() });
    waiting.splice(i, 1);
    sel = Math.min(sel, Math.max(0, waiting.length - 1));
    redraw();
    if (!arrived && waiting.length <= 1) {
      arrived = true;
      setTimeout(() => { waiting.push(later); redraw(waiting.length - 1); }, 1400);
    }
  }
  // Re-rendering replaces the rows, so focus that was in the desk is put back
  // on the selected row, and focus that was elsewhere is left alone.
  function redraw(newIdx) {
    const had = wEl.contains(document.activeElement) || document.activeElement === wEl;
    render(newIdx);
    if (!had) return;
    const f = document.getElementById('ask-' + sel);
    (f || wEl).focus({ preventScroll: true });
  }
  function move(d) {
    if (!waiting.length) return;
    sel = (sel + d + waiting.length) % waiting.length; redraw();
  }
  wEl.addEventListener('click', e => {
    const row = e.target.closest('.ask'); if (!row) return;
    const i = +row.dataset.i, btn = e.target.closest('.act');
    if (btn) { answer(i, +btn.dataset.k); return; }
    sel = i; render(); document.getElementById('ask-' + sel).focus({ preventScroll: true });
  });
  // Keys act only while focus is inside the desk, so a key pressed anywhere
  // else on the page never answers an ask.
  wEl.addEventListener('keydown', e => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'j' || e.key === 'ArrowDown') { e.preventDefault(); move(1); }
    else if (e.key === 'k' || e.key === 'ArrowUp') { e.preventDefault(); move(-1); }
    else if (e.key === '1' || e.key === '2') { e.preventDefault(); answer(sel, +e.key - 1); }
  });
  render();
  // Tour scenes: each loops while on screen; reduced motion shows the end state.
  const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const scenes = {
    async brain(el, live) {
      const ctx = el.querySelector('.ctx'), meter = el.querySelector('.meter'), bar = meter.querySelector('i'), pct = el.querySelector('.pct'), flag = el.querySelector('.flag');
      const rows = [...el.querySelectorAll('.kv div')];
      const lines = ['read issue 212', 'grep migrations/', 'plan: split in two', 'ran cargo test', 'diff store.rs', 'notes on rollback', 'tried a fix', 'reran the suite'];
      const set = p => { bar.style.width = p + '%'; pct.textContent = p + '%'; meter.classList.toggle('full', p > 85); };
      if (still) { rows.forEach(r => r.classList.add('on')); ctx.innerHTML = '<div class="back">plan: split the store migration in two</div><div class="back">next: write the rollback test</div>'; set(12); return; }
      while (live()) {
        ctx.classList.remove('wipe'); ctx.innerHTML = ''; rows.forEach(r => r.classList.remove('on', 'hot')); set(10); flag.classList.remove('on');
        for (let i = 0; i < lines.length && live(); i++) {
          const d = document.createElement('div'); d.className = 'in'; d.textContent = lines[i]; ctx.append(d); set(10 + Math.round((i + 1) * 11.2));
          if (i === 2 || i === 4 || i === 6) { const r = rows[(i - 2) / 2]; r.classList.add('on', 'hot'); setTimeout(() => r.classList.remove('hot'), 700); }
          await wait(620);
        }
        await wait(500); ctx.classList.add('wipe'); flag.classList.add('on'); set(6); await wait(1300);
        flag.classList.remove('on'); ctx.classList.remove('wipe'); ctx.innerHTML = '';
        rows.slice(0, 2).forEach(r => r.classList.add('hot'));
        for (const t of ['plan: split the store migration in two', 'next: write the rollback test']) { const d = document.createElement('div'); d.className = 'in back'; d.textContent = t; ctx.append(d); await wait(450); }
        set(12); await wait(900); rows.forEach(r => r.classList.remove('hot')); await wait(2600);
      }
    },
    async feed(el, live) {
      const box = el.querySelector('.events'), n = el.querySelector('.new b');
      const evs = [
        ['<b>release-bot</b> shipped 1.4.2 to the NAS', '09:41'],
        ['<b>researcher</b> published "Storage layouts"', '09:44'],
        ['<b>tidy</b> pruned 14 finished sessions', '09:52'],
        ['<b>reviewer</b> asked you about the rollback', '09:58'],
      ];
      const row = (h, t, cls = '') => { const d = document.createElement('div'); d.className = 'ev ' + cls; d.innerHTML = `<span>${h}</span><time>${t}</time>`; return d; };
      if (still) { box.innerHTML = ''; evs.slice().reverse().forEach(([h, t]) => box.append(row(h, t))); n.textContent = evs.length; return; }
      while (live()) {
        box.innerHTML = ''; n.textContent = 0;
        box.append(row('<b>nightly</b> finished the 02:00 run', '02:14', 'old'));
        for (let i = 0; i < evs.length && live(); i++) { await wait(1300); box.prepend(row(...evs[i])); n.textContent = i + 1; }
        await wait(1400);
        box.prepend(row('<b>a new agent</b> read the feed and started', '10:02', 'reader'));
        await wait(1200); box.querySelectorAll('.ev').forEach(e => e.classList.add('old')); n.textContent = 0;
        await wait(2600);
      }
    },
    async ask(el, live) {
      const slot = el.querySelector('.slot'), pos = el.querySelector('.pos b');
      const qs = [
        ['release-bot, homelab', 'Ship 1.4.2 to the NAS tonight?', ['Ship it', 'Hold until Monday'], 'Changelog and test run attached.'],
        ['researcher, notes', 'Which layout should I write up?', ['Layout B', 'Layout A'], 'Both drafts are in Artifacts.'],
        ['tidy, agents', 'Prune 14 finished sessions?', ['Prune', 'Keep them'], 'Project knowledge is not touched.'],
      ];
      const card = ([who, q, b, th]) => { const c = document.createElement('div'); c.className = 'card'; c.innerHTML = `<div class="who">${who}</div><div class="q">${q}</div><div class="btns"><span class="b p">${b[0]}</span><span class="b">${b[1]}</span></div><div class="sent">Answer sent. The agent reads it on its next turn.</div><div class="thread">${th}</div>`; return c; };
      if (still) { slot.innerHTML = ''; slot.append(card(qs[0])); return; }
      let i = 0;
      slot.innerHTML = ''; let cur = card(qs[0]); slot.append(cur);
      while (live()) {
        pos.textContent = i + 1;
        await wait(1900); const b = cur.querySelector('.b.p'); b.classList.add('press'); await wait(160); b.classList.remove('press');
        cur.querySelector('.sent').classList.add('on'); await wait(1100);
        i = (i + 1) % qs.length; const next = card(qs[i]); next.classList.add('enter'); slot.append(next);
        cur.classList.add('leave'); requestAnimationFrame(() => requestAnimationFrame(() => next.classList.remove('enter')));
        const old = cur; setTimeout(() => old.remove(), 600); cur = next;
      }
    },
    async art(el, live) {
      const a = el.querySelector('.anchor'), note = el.querySelector('.note'), rep = el.querySelector('.reply'), ver = el.querySelector('.ver'), vb = ver.querySelector('b');
      if (still) { a.classList.add('on'); note.classList.add('on'); rep.classList.add('on'); vb.textContent = 2; return; }
      while (live()) {
        [a, note, rep].forEach(x => x.classList.remove('on')); vb.textContent = 1; ver.classList.remove('bump');
        await wait(1200); a.classList.add('on'); await wait(900); note.classList.add('on');
        await wait(1700); rep.classList.add('on'); await wait(500); vb.textContent = 2; ver.classList.add('bump');
        await wait(3400);
      }
    },
  };
  document.querySelectorAll('.stage[data-scene]').forEach(st => {
    let on = false, running = false;
    new IntersectionObserver(([en]) => {
      on = en.isIntersecting;
      if (on && !running) { running = true; scenes[st.dataset.scene](st, () => on).finally(() => { running = false; }); }
    }, { threshold: 0.35 }).observe(st);
  });
})();
