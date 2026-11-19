// Libretto view: the scrolling sung text for one zone, synchronised to playback.
//
// A host page owns the WebSocket and decides which zone to follow; this file only
// renders. Use:
//
//   const view = LibrettoView.create(rootElement, {
//       scroller: window,        // what scrolls the list (window, or a scrolling element)
//       onUpdate: state => {},   // called whenever the position or content changes
//   });
//   view.setContent(content);    // LibrettoContent from the server, or null (no match)
//   view.setZone(zone);          // a now_playing zone: state, position, length
//   view.reportPosition(secs);   // a seek_updated position for that zone
//   view.resetPosition(secs);    // switching zones: restart position tracking
//   view.setOption(name, val);   // 'translation' | 'ensemble' | 'bars'; remembered in this browser
//   view.options; view.state();
//
// Several views can live on one page: element ids are prefixed per view.
(function () {
    'use strict';

    // Roon reports position in whole seconds, about once a second. Between reports we
    // count forward locally so lines change on time instead of up to a second late.
    const MAX_EXTRAPOLATE_S = 2.0;  // never run more than this past the last report
    // Show each line slightly before it is sung so the reader keeps up (?lead=seconds).
    const LEAD_SECONDS = (() => {
        const v = parseFloat(new URLSearchParams(location.search).get('lead'));
        return Number.isFinite(v) ? v : 0.7;
    })();

    // ---- Ensemble passages ----
    // Sung-text tracks give each line an `end` (its last note), optionally a `voice`
    // (score order), and a `gloss` with `verses` (the character's text, once). Where two
    // or more characters sing at once, the passage is shown as a card in the list:
    //   stanza: each character's verses for the whole passage, fixed; the phrases being
    //           sung now are highlighted, so repeats move the highlight, not the text
    //   rows:   each character's last few lines, newest at the bottom
    // Passages are worked out once per track, in display time (position + lead).
    // Tracks without `end` times (librettos) never use cards.
    const ENS_ENTER_S = 0.5;   // two voices must overlap this long to start a passage
    const ENS_EXIT_S = 2.5;    // overlaps closer together than this are one passage
    const ENS_TAIL_S = 0.5;    // a passage ends this long after its last overlap
    const ENS_HOLD_S = 1.0;    // a line stays lit this long after its last note (short rests)
    const ENS_HISTORY = 3;     // lines per character in 'rows' style

    const OPTS_KEY = 'librettoViewOpts';
    let viewCount = 0;

    function escapeHtml(text) {
        return String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    function formatTime(secs) {
        if (secs == null || isNaN(secs)) return '--:--';
        const s = Math.floor(secs);
        return `${Math.floor(s / 60)}:${(s % 60).toString().padStart(2, '0')}`;
    }

    // ---- Where a line is in the score ----
    // Sung-text files carry the piece id ("no16", "no18-aria", "rec05a", "app-no15") and
    // bar_start/bar_end as printed bar + fraction (20.39 = 39% into bar 20).
    function pieceLabel(id) {
        if (!id) return '';
        let m = id.match(/^no(\d+)(?:-(recit|aria))?$/);
        if (m) return `No. ${+m[1]}` + (m[2] === 'recit' ? ' recit.' : m[2] === 'aria' ? ' aria' : '');
        // A recitative after No. N: "Recit. 20b" (the second after No. 20), short enough
        // for the narrow column under the time and singer.
        m = id.match(/^rec(\d+)([a-z])$/);
        if (m) return `Recit. ${+m[1]}${m[2]}`;
        m = id.match(/^app-no(\d+)([a-z]?)$/);
        if (m) return `Appendix No. ${+m[1]}${m[2]}`;
        return id;
    }

    function barsLabel(b0, b1) {
        if (b0 == null) return '';
        const a = Math.floor(b0), b = Math.max(a, Math.floor(b1 ?? b0));
        return a === b ? `bar ${a}` : `bars ${a}–${b}`;
    }

    // "No. 16 · bars 245–247" for a list of lines (one line, or an ensemble card's lines).
    function whereLabel(lines) {
        const piece = lines.map(s => s.piece).find(Boolean);
        const bars = lines.filter(s => s.bar_start != null);
        const b0 = bars.length ? Math.min(...bars.map(s => s.bar_start)) : null;
        const b1 = bars.length ? Math.max(...bars.map(s => s.bar_end ?? s.bar_start)) : null;
        return [pieceLabel(piece), barsLabel(b0, b1)].filter(Boolean).join(' · ');
    }

    function create(root, config = {}) {
        const scroller = config.scroller || window;
        const onUpdate = config.onUpdate || (() => {});
        const MSG = Object.assign({
            waiting: 'Waiting for playback...',
            noMatch: 'No libretto match for this track',
            noSegments: 'No libretto segments for this track',
        }, config.messages || {});
        const P = `lv${++viewCount}-`;  // element id prefix
        const byId = id => document.getElementById(P + id);
        const empty = text => `<div class="lv-empty">${escapeHtml(text)}</div>`;
        root.classList.add('lv');
        root.innerHTML = empty(MSG.waiting);

        // ---- State ----
        let currentContent = null;   // LibrettoContent from server (segments, metadata)
        let currentSegmentIndex = -1;
        let seekPosition = 0;
        let trackLength = 0;
        // reportedPos/reportedAt anchor the position estimate; isPlaying freezes it when paused.
        let reportedPos = 0;
        let reportedAt = 0;
        let isPlaying = false;
        let lastReport = null;
        let ens = null;  // { order, byChar, passages: [{t0, t1, rows: [{c, lines, verses}]}], verses }
        let flow = [];          // list items in order: {kind: 'seg'|'card', start, id, i | pi, key}
        let flowActive = -2;    // index into flow of the active item
        let markedLine = null;
        let segmentGroupMap = [];  // segment index -> { start, end } of its group, or null
        let autoScroll = true;

        // ---- Display options (remembered in this browser) ----
        // translation: 'aligned' (each Italian phrase over its literal English), 'line', 'off'
        // ensemble:    'stanza' (each character's text once, the highlight moves within it)
        //              or 'rows' (each character's most recent lines)
        // bars:        'on' shows each line's piece and bars under the singer's name, 'off'
        const OPTS = { translation: 'aligned', ensemble: 'stanza', bars: 'off' };
        try { Object.assign(OPTS, JSON.parse(localStorage.getItem(OPTS_KEY) || '{}')); } catch (e) {}
        function saveOpts() { try { localStorage.setItem(OPTS_KEY, JSON.stringify(OPTS)); } catch (e) {} }

        function estimatedPosition() {
            if (!isPlaying) return reportedPos;
            const elapsed = (performance.now() - reportedAt) / 1000;
            return reportedPos + Math.min(Math.max(elapsed, 0), MAX_EXTRAPOLATE_S);
        }

        // A report of N means the true position is somewhere in [N, N+1). Keep the running
        // estimate if it is inside that second; otherwise (seek, drift, first report)
        // re-anchor at the middle of it.
        function onReportedPosition(seconds) {
            const r = Math.max(0, seconds || 0);
            // Reports advancing by 1-2 s mean playback is running, even before a zone update says so.
            if (!isPlaying && lastReport !== null && r > lastReport && r - lastReport <= 2) {
                isPlaying = true;
            }
            lastReport = r;
            const est = estimatedPosition();
            if (!(isPlaying && est >= r && est < r + 1)) {
                reportedPos = r + (isPlaying ? 0.5 : 0);
                reportedAt = performance.now();
            }
            seekPosition = estimatedPosition();
            updateHighlight(seekPosition);
        }

        const ticker = setInterval(() => {
            if (currentContent && isPlaying) {
                seekPosition = estimatedPosition();
                updateHighlight(seekPosition);
            }
        }, 100);

        // gloss: [{v, c, it, en}] phrases; hl: Set of "v|c" phrase keys to highlight (optional)
        function renderGloss(gloss, hl) {
            return '<div class="gloss">' + gloss.map(g => {
                const on = hl && hl.has(`${g.v}|${g.c}`) ? ' now' : '';
                return `<span class="gl${on}"><span class="gl-it">${escapeHtml(g.it)}</span>` +
                       `<span class="gl-en">${escapeHtml(g.en)}</span></span>`;
            }).join('') + '</div>';
        }

        // One sung line, according to the translation option.
        function renderSung(seg) {
            if (OPTS.translation === 'aligned' && seg.gloss && seg.gloss.length) return renderGloss(seg.gloss);
            let html = `<div class="text">${escapeHtml(seg.text || '')}</div>`;
            if (OPTS.translation !== 'off') html += `<div class="translation">${escapeHtml(seg.translation || '')}</div>`;
            return html;
        }

        // One whole verse of a character's text ([[it, en], ...]), highlighting phrases in hl.
        function renderVerse(vid, chunks, hl) {
            const gloss = chunks.map(([it, en], c) => ({ v: vid, c, it, en }));
            if (OPTS.translation === 'aligned') return renderGloss(gloss, hl);
            const span = (g, field) => `<span${hl && hl.has(`${g.v}|${g.c}`) ? ' class="now"' : ''}>${escapeHtml(g[field])}</span>`;
            let html = `<div class="text">${gloss.map(g => span(g, 'it')).join(' ')}</div>`;
            if (OPTS.translation !== 'off') html += `<div class="translation">${gloss.map(g => span(g, 'en')).join(' ')}</div>`;
            return html;
        }

        function prepareEnsemble(segments) {
            ens = null;
            const lines = (segments || []).filter(s => s.end != null && s.character);
            if (!lines.length) return;
            const byChar = {}, first = {}, voice = {}, verses = {};
            lines.forEach((s, i) => {
                (byChar[s.character] = byChar[s.character] || []).push(s);
                if (!(s.character in first)) first[s.character] = i;
                if (s.voice != null) voice[s.character] = s.voice;
                Object.assign(verses, s.verses || {});
            });
            for (const c in byChar) byChar[c].sort((a, b) => a.start - b.start);
            const order = Object.keys(byChar).sort((a, b) =>
                ((voice[a] ?? 1e6) - (voice[b] ?? 1e6)) || (first[a] - first[b]));

            // Passages: from when two voices start overlapping (for at least ENS_ENTER_S) to
            // just after the last overlap (ENS_TAIL_S). Overlaps less than ENS_EXIT_S apart
            // belong to the same passage, so a brief solo doesn't split an ensemble.
            const tMax = Math.max(...lines.map(s => s.end)) + ENS_EXIT_S + 1;
            const passages = [];
            let on = false, overlapSince = null, quietSince = null, t0 = 0, lastOverlap = 0;
            for (let i = 0; i * 0.1 <= tMax; i++) {
                const t = i * 0.1;
                const sounding = new Set(lines.filter(s => s.start <= t && t < s.end).map(s => s.character)).size;
                if (sounding >= 2) {
                    lastOverlap = t;
                    quietSince = null;
                    if (overlapSince === null) overlapSince = t;
                    if (!on && t - overlapSince >= ENS_ENTER_S) { on = true; t0 = overlapSince; }
                } else {
                    overlapSince = null;
                    if (on) {
                        if (quietSince === null) quietSince = t;
                        if (t - quietSince >= ENS_EXIT_S) { on = false; passages.push({ t0, t1: lastOverlap + ENS_TAIL_S }); }
                    }
                }
            }
            if (on) passages.push({ t0, t1: lastOverlap + ENS_TAIL_S });
            const verseNo = vid => parseInt(vid.slice(vid.lastIndexOf(':') + 1), 10);
            for (const p of passages) {
                p.rows = [];
                const members = new Set();
                for (const c of order) {
                    // Lines in the passage. One that merely ends just after it starts belongs to
                    // what came before (unless it starts with the overlap that opened the
                    // passage); one that starts just before it ends and runs on is the next
                    // solo, not part of the ensemble.
                    const ls = byChar[c].filter(s => s.start < p.t1
                        && (s.start >= p.t0 - ENS_ENTER_S || s.end > p.t0 + 1.0)
                        && !(s.start >= p.t1 - 1.0 && s.end > p.t1 + 1.0));
                    if (!ls.length) continue;
                    ls.forEach(s => members.add(s));
                    const vids = [...new Set(ls.flatMap(s => (s.gloss || []).map(g => g.v)))]
                        .filter(v => verses[v]).sort((a, b) => verseNo(a) - verseNo(b));
                    p.rows.push({ c, lines: ls, verses: vids });
                }
                // The card starts with its earliest line. Short lines that start inside the
                // card's span but belong to what came before (e.g. the tail of a solo phrase)
                // are absorbed: shown neither as a list line nor in the stanza.
                p.start = Math.min(p.t0, ...[...members].map(s => s.start));
                p.absorbed = lines.filter(s => !members.has(s) && s.start >= p.start && s.start < p.t0);
            }
            // A brief overlap whose lines mostly belong elsewhere can leave a single voice:
            // that is not an ensemble, so its lines stay in the list.
            ens = { order, byChar, passages: passages.filter(p => p.rows.length >= 2), verses };
        }

        // Latest line of `char` that has started by q, and the next one after it.
        function ensLines(char, q) {
            const list = ens.byChar[char];
            let cur = null, next = null;
            for (const s of list) {
                if (s.start <= q) cur = s; else { next = s; break; }
            }
            return { cur, next };
        }

        // Card rows at display time q. live=false renders a still card (nobody highlighted).
        function cardRows(p, q, live) {
            return p.rows.map(r => {
                const { cur, next } = ensLines(r.c, q);
                let state = live ? 'resting' : 'still';
                if (live && cur && cur.end > p.t0 && q < cur.end + ENS_HOLD_S) state = 'singing';
                else if (live && next && next.start < p.t1 && next.start - q <= 2.0) state = 'next';
                return { ...r, cur, next, state };
            });
        }

        // Indices of a line's phrases being sung at q. A phrase is lit from its start until
        // ENS_HOLD_S after its end (bridging short rests), but no later than the start of
        // the line's next phrase. Returns null for lines without phrase times (older files).
        function litPhraseIndices(line, q) {
            const gl = line.gloss || [];
            if (!gl.length || gl[0].start == null) return null;
            const lit = new Set();
            gl.forEach((g, i) => {
                const nxt = gl.slice(i + 1).find(h => h.start > g.start);
                const until = Math.min(g.end + ENS_HOLD_S, nxt ? nxt.start : Infinity);
                if (g.start <= q && q < until) lit.add(i);
            });
            return lit;
        }

        // The same as "v|c" keys, for the stanza view (older files light every phrase).
        function litPhrases(line, q) {
            const gl = line.gloss || [];
            const idx = litPhraseIndices(line, q);
            return new Set(gl.filter((g, i) => !idx || idx.has(i)).map(g => `${g.v}|${g.c}`));
        }

        // In the list, the active solo line also lights the phrase being sung (aligned
        // translation only, where phrases are shown separately).
        function markLinePhrases(f, q) {
            if (markedLine && markedLine !== f) {
                const old = byId(markedLine.id);
                if (old) old.querySelectorAll('.gl.now').forEach(e => e.classList.remove('now'));
            }
            markedLine = f;
            if (!f) return;
            const el = byId(f.id);
            if (!el) return;
            const idx = litPhraseIndices(currentContent.segments[f.i], q);
            el.querySelectorAll('.gl').forEach((e, i) => e.classList.toggle('now', !!idx && idx.has(i)));
        }

        function renderCardBody(p, q, live) {
            const rows = cardRows(p, q, live);
            const lit = r => r.state === 'singing' && r.cur ? [...litPhrases(r.cur, q)].join(',') : '';
            const key = [OPTS.ensemble, OPTS.translation, live,
                         ...rows.map(r => `${r.c}|${r.cur ? r.cur.start : ''}|${r.state}|${lit(r)}`)].join(';');
            let html = '';
            for (const r of rows) {
                html += `<div class="ens-row ${OPTS.ensemble} ${r.state}">` +
                        `<div class="character">${escapeHtml(r.c)}</div><div class="ens-body">`;
                if (OPTS.ensemble === 'stanza' && r.verses.length) {
                    const hl = r.state === 'singing' && r.cur ? litPhrases(r.cur, q) : null;
                    html += r.verses.map(vid =>
                        `<div class="ens-verse">${renderVerse(vid, ens.verses[vid], hl)}</div>`).join('');
                } else {
                    const started = r.lines.filter(s => s.start <= q);
                    if (!started.length) {
                        html += `<div class="ens-line upcoming">${renderSung(r.next || r.lines[0])}</div>`;
                    } else {
                        const recent = started.slice(-ENS_HISTORY);
                        html += recent.map((s, i) => {
                            const latest = i === recent.length - 1;
                            return `<div class="ens-line${latest ? '' : ' older'}">${renderSung(s)}</div>`;
                        }).join('');
                    }
                }
                html += '</div></div>';
            }
            return { html, key };
        }

        function renderCard(f, q, live) {
            const body = renderCardBody(ens.passages[f.pi], q, live);
            if (body.key === f.key) return;
            f.key = body.key;
            const el = byId(f.id);
            if (el) el.querySelector('.ens-card-body').innerHTML = body.html;
        }

        // New text (another track, other display options) jumps straight to the current
        // line, even while auto-scroll is paused; after that the list scrolls smoothly.
        let jumpNext = true;
        function scrollTo(el, block) {
            if (!el || !(autoScroll || jumpNext)) return;
            el.scrollIntoView({ behavior: jumpNext ? 'auto' : 'smooth', block });
            jumpNext = false;
        }

        // Sung-text tracks: activate the list item (line or card) due at this position.
        function updateFlow(position) {
            const q = position + LEAD_SECONDS;
            let ai = -1;
            for (let k = 0; k < flow.length; k++) {
                if (flow[k].start <= q) ai = k; else break;
            }
            // Inside an ensemble passage its card stays active, whatever starts meanwhile.
            const inside = flow.findIndex(f => f.kind === 'card' && f.start <= q && q < ens.passages[f.pi].t1);
            if (inside >= 0) ai = inside;
            if (ai !== flowActive) {
                flowActive = ai;
                flow.forEach((f, k) => {
                    const el = byId(f.id);
                    if (!el) return;
                    const st = k < ai ? 'past' : k === ai ? 'active' : 'future';
                    el.classList.remove('active', 'past', 'future');
                    el.classList.add(st);
                    if (f.kind === 'card' && st !== 'active') {
                        const p = ens.passages[f.pi];
                        renderCard(f, st === 'past' ? p.t1 : p.t0 - 0.001, false);
                    }
                });
                currentSegmentIndex = ai >= 0 && flow[ai].kind === 'seg' ? flow[ai].i : -1;
                if (ai >= 0) scrollTo(byId(flow[ai].id), flow[ai].kind === 'card' ? 'start' : 'center');
            }
            if (ai >= 0 && flow[ai].kind === 'card') {
                const p = ens.passages[flow[ai].pi];
                if (q < p.t1) renderCard(flow[ai], q, true);
                else renderCard(flow[ai], p.t1, false);
            }
            markLinePhrases(ai >= 0 && flow[ai].kind === 'seg' ? flow[ai] : null, q);
        }

        // Find the active segment index for a given position
        function findSegmentIndex(segments, position) {
            let idx = -1;
            for (let i = 0; i < segments.length; i++) {
                if (segments[i].start <= position) idx = i; else break;
            }
            return idx;
        }

        // Build group ranges: consecutive runs of segments sharing the same group tag
        // (older word-count librettos).
        function buildGroupMap(segments) {
            const map = new Array(segments.length).fill(null);
            let i = 0;
            while (i < segments.length) {
                const g = segments[i].group;
                if (g) {
                    const start = i;
                    while (i < segments.length && segments[i].group === g) i++;
                    const end = i - 1;
                    if (end > start) {  // only a group if 2+ segments
                        for (let j = start; j <= end; j++) map[j] = { start, end };
                    }
                } else {
                    i++;
                }
            }
            return map;
        }

        function renderSegmentInner(seg, isGroupCont) {
            let html = `<div class="${isGroupCont ? 'seg-start group-cont' : 'seg-start'}">${formatTime(seg.start)}</div>`;
            html += `<div class="character">${escapeHtml(seg.character || '')}</div>`;
            // With Bars on, the time and singer sit in a block with the piece and bars
            // under them, level with the second line of text (the translation).
            const where = OPTS.bars === 'on' ? whereLabel([seg]) : '';
            if (where) html = `<div class="lv-lead"><div class="lv-lead-top">${html}</div>` +
                              `<div class="lv-where">${escapeHtml(where)}</div></div>`;
            html += `<div class="texts">`;
            html += seg.direction ? `<div class="direction-text">${escapeHtml(seg.direction)}</div>` : renderSung(seg);
            html += `</div>`;
            return html;
        }

        function renderSegments(segments) {
            root.classList.toggle('lv-bars', OPTS.bars === 'on');
            if (!segments || segments.length === 0) {
                root.innerHTML = empty(MSG.noSegments);
                segmentGroupMap = [];
                flow = [];
                return;
            }
            segmentGroupMap = buildGroupMap(segments);

            // Sung-text tracks: lines that belong to an ensemble passage are shown in that
            // passage's card, placed where the passage's first line would be.
            flow = [];
            flowActive = -2;
            markedLine = null;
            const inCard = new Set(), cardAt = new Map();
            if (ens) {
                ens.passages.forEach((p, pi) => {
                    const idxs = [...p.rows.flatMap(r => r.lines), ...p.absorbed]
                        .map(s => segments.indexOf(s)).filter(x => x >= 0);
                    if (!idxs.length) return;
                    idxs.forEach(x => inCard.add(x));
                    cardAt.set(Math.min(...idxs), pi);
                });
            }

            let html = '';
            let i = 0;
            while (i < segments.length) {
                if (cardAt.has(i)) {
                    const pi = cardAt.get(i), p = ens.passages[pi];
                    const f = { kind: 'card', pi, id: `card-${pi}`, start: p.start, key: '' };
                    const body = renderCardBody(p, p.t0 - 0.001, false);
                    f.key = body.key;
                    const where = OPTS.bars === 'on' ? whereLabel(p.rows.flatMap(r => r.lines)) : '';
                    html += `<div class="ens-card future" id="${P}${f.id}"><div class="ens-title">Ensemble` +
                            `<span class="ens-span">${formatTime(p.t0 - LEAD_SECONDS)} – ${formatTime(p.t1 - LEAD_SECONDS)}</span>` +
                            (where ? `<span class="ens-span lv-where">${escapeHtml(where)}</span>` : '') + `</div>` +
                            `<div class="ens-card-body">${body.html}</div></div>`;
                    flow.push(f);
                }
                if (inCard.has(i)) { i++; continue; }
                const grp = segmentGroupMap[i];
                if (grp && i === grp.start) {
                    html += `<div class="ensemble-group future" id="${P}group-${grp.start}">`;
                    for (let j = grp.start; j <= grp.end; j++) {
                        html += `<div class="segment future" data-index="${j}" id="${P}seg-${j}">`;
                        html += renderSegmentInner(segments[j], j > grp.start);
                        html += `</div>`;
                    }
                    html += `</div>`;
                    i = grp.end + 1;
                } else {
                    html += `<div class="segment future" data-index="${i}" id="${P}seg-${i}">`;
                    html += renderSegmentInner(segments[i], false);
                    html += `</div>`;
                    if (ens) flow.push({ kind: 'seg', i, id: `seg-${i}`, start: segments[i].start });
                    i++;
                }
            }
            flow.sort((a, b) => a.start - b.start);
            root.innerHTML = html;
        }

        // Older librettos (no line end times): one active line, or a whole group.
        function updateSegments(position) {
            const newIndex = findSegmentIndex(currentContent.segments, position + LEAD_SECONDS);
            if (newIndex === currentSegmentIndex) return;
            currentSegmentIndex = newIndex;

            const grp = segmentGroupMap[newIndex];
            const activeStart = grp ? grp.start : newIndex;
            const activeEnd = grp ? grp.end : newIndex;
            root.querySelectorAll('.segment[data-index]').forEach(el => {
                const i = parseInt(el.dataset.index);
                el.classList.remove('active', 'past', 'future');
                el.classList.add(i < activeStart ? 'past' : i <= activeEnd ? 'active' : 'future');
            });
            root.querySelectorAll('.ensemble-group').forEach(el => {
                el.classList.remove('active', 'past', 'future');
                const firstSeg = el.querySelector('.segment[data-index]');
                const g = firstSeg && segmentGroupMap[parseInt(firstSeg.dataset.index)];
                if (g) el.classList.add(g.end < activeStart ? 'past' : g.start > activeEnd ? 'future' : 'active');
            });
            if (newIndex >= 0) scrollTo(grp ? byId(`group-${grp.start}`) : byId(`seg-${newIndex}`), 'center');
        }

        function updateHighlight(position) {
            if (!currentContent) return;
            if (ens) updateFlow(position); else updateSegments(position);
            onUpdate(state());
        }

        // What the host shows around the text: time, progress, debug details.
        function state() {
            const segs = currentContent?.segments || [];
            const active = flowActive >= 0 ? flow[flowActive] : null;
            return {
                content: currentContent,
                position: seekPosition,
                length: trackLength,
                playing: isPlaying,
                segmentIndex: currentSegmentIndex,     // -1 when none (or inside a card)
                segmentCount: segs.length,
                segment: currentSegmentIndex >= 0 ? segs[currentSegmentIndex] : null,
                nextSegment: currentSegmentIndex >= 0 ? segs[currentSegmentIndex + 1] || null : null,
                inEnsemble: !!(ens && active && active.kind === 'card'),
            };
        }

        function setContent(content) {
            currentContent = content || null;
            currentSegmentIndex = -1;
            jumpNext = true;
            if (!currentContent) {
                prepareEnsemble(null);
                flow = [];
                root.innerHTML = empty(MSG.noMatch);
                onUpdate(state());
                return;
            }
            prepareEnsemble(currentContent.segments);  // before rendering: the list places cards
            renderSegments(currentContent.segments);
            seekPosition = estimatedPosition();
            if (seekPosition > 0) updateHighlight(seekPosition); else onUpdate(state());
        }

        // A now_playing zone update for the zone this view follows.
        function setZone(zone) {
            trackLength = zone.length_seconds || trackLength;
            if (zone.state !== 'Playing') {
                if (isPlaying) {
                    reportedPos = estimatedPosition();  // freeze where we are
                    isPlaying = false;
                }
                onUpdate(state());
                return;
            }
            if (!isPlaying) {
                isPlaying = true;
                reportedAt = performance.now();
            }
            if (zone.position_seconds != null) onReportedPosition(zone.position_seconds);
        }

        // Following a different zone: start position tracking again from its position.
        function resetPosition(seconds) {
            lastReport = null;
            isPlaying = false;
            reportedPos = seconds || 0;
            reportedAt = performance.now();
        }

        // Jump back to the current line (after the host changes the text's size).
        function recenter() {
            jumpNext = true;
            flowActive = -2;
            currentSegmentIndex = -1;
            if (currentContent) updateHighlight(seekPosition);
        }

        function setOption(name, val) {
            if (!(name in OPTS)) return;
            OPTS[name] = val;
            saveOpts();
            jumpNext = true;
            if (currentContent) {
                renderSegments(currentContent.segments);
                currentSegmentIndex = -1;
                updateHighlight(seekPosition);
            }
        }

        // Scrolling by hand (touch, wheel, scrollbar, keys) pauses auto-scroll; it resumes
        // 5 s after the last such input. Scroll events themselves aren't used: the view's
        // own smooth scrolling fires them too, and would pause it after every move.
        let scrollTimer = null;
        const onUserScroll = () => {
            autoScroll = false;
            clearTimeout(scrollTimer);
            scrollTimer = setTimeout(() => { autoScroll = true; }, 5000);
        };
        const onKey = e => {
            if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(e.key)) onUserScroll();
        };
        const target = scroller === window ? document : scroller;
        const INPUTS = ['wheel', 'touchmove', 'pointerdown'];
        INPUTS.forEach(t => target.addEventListener(t, onUserScroll, { passive: true }));
        window.addEventListener('keydown', onKey);

        function destroy() {
            clearInterval(ticker);
            clearTimeout(scrollTimer);
            INPUTS.forEach(t => target.removeEventListener(t, onUserScroll));
            window.removeEventListener('keydown', onKey);
            root.innerHTML = '';
            root.classList.remove('lv');
        }

        return {
            setContent, setZone, resetPosition, setOption, state, destroy, recenter,
            reportPosition: onReportedPosition,
            get options() { return { ...OPTS }; },
        };
    }

    window.LibrettoView = { create, formatTime, escapeHtml, LEAD_SECONDS };
})();
