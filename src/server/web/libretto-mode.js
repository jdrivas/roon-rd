// Libretto mode for the main page: one zone's sung text, either beside its now-playing
// controls ("split") or filling the display ("full").
//
//   normal zone view --[Libretto]--> split --[expand]--> full
//                    <--[back]------       <--[collapse]--
//
// The main page passes on what it receives: LibrettoMode.onZones(now_playing),
// .onLibrettoUpdate(msg), .onSeek(msg), and calls .decorate() after drawing its zone
// cards, which adds a Libretto button to each card whose track has a libretto.
//
// Leaving by itself: the mode stays up while the zone's track has a libretto (even one
// with no sung text, like an overture) or the album is still the one last seen with a
// libretto. Otherwise it returns to the normal view after GRACE_MS, so the gap between
// two tracks doesn't make it flicker.
//
// The mode is kept in the URL (#libretto=<zone> or #libretto-full=<zone>), so a reload
// or a bookmark reopens it. Uses the main page's sendControl, toggleMute, handleSeek,
// toggleFullscreen and formatTrackTitle.
(function () {
    'use strict';

    const GRACE_MS = 3000;
    const SCALE_KEY = 'librettoModeScale';
    const SCALES = [0.7, 0.8, 0.9, 1, 1.12, 1.25, 1.4, 1.6];
    const { formatTime, escapeHtml } = LibrettoView;

    const contentByZone = {};  // zone_id -> LibrettoContent or null
    let zones = [];            // latest now_playing list
    let lm = null;             // the open mode: { zoneId, full, album, view, el, leaveTimer, last }
    let pending = parseHash(); // a mode to reopen once its zone is known
    let scaleIdx = 3;
    try {
        const v = parseInt(localStorage.getItem(SCALE_KEY), 10);
        if (v >= 0 && v < SCALES.length) scaleIdx = v;
    } catch (e) {}

    const svg = body => `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;
    const ICON = {
        libretto: svg('<path d="M8 4.5C6 3.4 4 3.4 2 4.3v8.4c2-.9 4-.9 6 .2"/><path d="M8 4.5c2-1.1 4-1.1 6-.2v8.4c-2-.9-4-.9-6 .2z"/>'),
        back: svg('<line x1="4" y1="4" x2="12" y2="12"/><line x1="12" y1="4" x2="4" y2="12"/>'),
        settings: svg('<line x1="2" y1="4" x2="14" y2="4"/><line x1="2" y1="8" x2="14" y2="8"/><line x1="2" y1="12" x2="14" y2="12"/><circle cx="10" cy="4" r="1.6" fill="currentColor"/><circle cx="5" cy="8" r="1.6" fill="currentColor"/><circle cx="11" cy="12" r="1.6" fill="currentColor"/>'),
        info: svg('<circle cx="8" cy="8" r="6"/><line x1="8" y1="7.5" x2="8" y2="11"/><circle cx="8" cy="5" r=".6" fill="currentColor"/>'),
        expand: svg('<path d="M9.5 2.5h4v4"/><path d="M13.5 2.5l-4.5 4.5"/><path d="M6.5 13.5h-4v-4"/><path d="M2.5 13.5l4.5-4.5"/>'),
        collapse: svg('<path d="M13.5 2.5l-4 4"/><path d="M9.5 3v3.5H13"/><path d="M2.5 13.5l4-4"/><path d="M6.5 13V9.5H3"/>'),
        prev: svg('<path d="M11 3L5 8l6 5V3z"/><line x1="4" y1="3" x2="4" y2="13"/>'),
        next: svg('<path d="M5 3l6 5-6 5V3z"/><line x1="12" y1="3" x2="12" y2="13"/>'),
        play: '<svg viewBox="0 0 16 16" fill="currentColor" stroke="none"><path d="M5 3l8 5-8 5V3z"/></svg>',
        pause: svg('<rect x="5" y="3" width="2" height="10"/><rect x="9" y="3" width="2" height="10"/>'),
        sound: svg('<path d="M8 3L5 6H2v4h3l3 3V3z"/><path d="M11 6.5c.43 1.1.43 2.3 0 3.4"/><path d="M13.5 4.5c1 2 1 5.5 0 7.5"/>'),
        muted: svg('<path d="M8 3L5 6H2v4h3l3 3V3z"/><line x1="11" y1="6" x2="14" y2="9"/><line x1="14" y1="6" x2="11" y2="9"/>'),
    };

    // ---- URL ----
    function parseHash() {
        const m = location.hash.match(/^#libretto(-full)?=(.+)$/);
        return m ? { zoneId: decodeURIComponent(m[2]), full: !!m[1] } : null;
    }
    function writeHash() {
        const h = lm ? `#libretto${lm.full ? '-full' : ''}=${encodeURIComponent(lm.zoneId)}` : '';
        if (h !== location.hash) history.replaceState(null, '', h || location.pathname + location.search);
    }
    window.addEventListener('hashchange', () => {
        const h = parseHash();
        if (!h) { if (lm) close(); return; }
        if (lm && lm.zoneId === h.zoneId) { setFull(h.full); return; }
        pending = h;
        tryPending();
    });

    // ---- Zone cards ----
    // A Libretto button on each card whose track has a libretto (cards are redrawn by the
    // main page, so this runs after every redraw).
    function decorate() {
        document.querySelectorAll('.zone[data-zone-id]').forEach(card => {
            const id = card.getAttribute('data-zone-id');
            const controls = card.querySelector('.zone-controls');
            if (!controls) return;
            let btn = controls.querySelector('.lm-open');
            const has = !!contentByZone[id];
            if (has && !btn) {
                btn = document.createElement('button');
                btn.className = 'control-btn lm-open';
                btn.title = 'Libretto';
                btn.innerHTML = ICON.libretto;
                btn.addEventListener('click', () => open(id, false));
                controls.insertBefore(btn, controls.firstChild);
            } else if (!has && btn) {
                btn.remove();
            }
        });
    }

    // ---- Messages from the main page ----
    function onLibrettoUpdate(msg) {
        contentByZone[msg.zone_id] = msg.content || null;
        decorate();
        if (lm && msg.zone_id === lm.zoneId) {
            lm.view.setContent(msg.content || null);
            renderInfo();
            checkKeep();
        }
        tryPending();
    }

    function onZones(list) {
        zones = list || [];
        if (lm) {
            const zone = currentZone();
            if (zone) {
                lm.view.setZone(zone);
                renderSide(zone);
            }
            checkKeep();
        }
        tryPending();
    }

    function onSeek(msg) {
        if (!lm || msg.zone_id !== lm.zoneId) return;
        lm.view.reportPosition(msg.seek_position || 0);
        if (!contentByZone[lm.zoneId]) {  // no view updates without content: show the time here
            const zone = currentZone();
            renderTimes(msg.seek_position || 0, zone ? zone.length_seconds : 0);
        }
    }

    function currentZone() {
        return lm ? zones.find(z => z.zone_id === lm.zoneId) || null : null;
    }

    function tryPending() {
        if (!pending || !zones.length) return;
        const p = pending;
        pending = null;
        if (zones.some(z => z.zone_id === p.zoneId)) open(p.zoneId, p.full);
        else writeHash();
    }

    // Stay while the track has a libretto, or the album is the one last seen with one.
    function checkKeep() {
        if (!lm) return;
        const zone = currentZone();
        const content = contentByZone[lm.zoneId];
        if (content && zone && zone.album) lm.album = zone.album;
        const keep = !!content || !!(zone && zone.album && zone.album === lm.album);
        if (keep) {
            clearTimeout(lm.leaveTimer);
            lm.leaveTimer = null;
        } else if (!lm.leaveTimer) {
            lm.leaveTimer = setTimeout(() => { if (lm) close(); }, GRACE_MS);
        }
    }

    // ---- Opening and closing ----
    function open(zoneId, full) {
        if (lm) close();
        const zone = zones.find(z => z.zone_id === zoneId) || null;
        const el = document.createElement('div');
        el.className = 'lm';
        el.innerHTML = `
            <aside class="lm-side">
                <div class="lm-art-wrap"><img class="lm-art" alt=""></div>
                <div class="lm-meta">
                    <div class="lm-opera"></div>
                    <div class="lm-track"></div>
                    <div class="lm-artist"></div>
                </div>
                <div class="lm-play">
                    <div class="lm-progress"><div class="lm-bar"><div class="lm-fill"></div></div></div>
                    <div class="lm-times"><span class="lm-cur"></span><span class="lm-zone"></span><span class="lm-len"></span></div>
                    <div class="lm-transport">
                        <button class="lm-btn" data-act="previous" title="Previous">${ICON.prev}</button>
                        <button class="lm-btn lm-pp" data-act="playpause" title="Play / pause"></button>
                        <button class="lm-btn" data-act="next" title="Next">${ICON.next}</button>
                        <button class="lm-btn lm-mute" data-act="mute" title="Mute"></button>
                    </div>
                </div>
                <div class="lm-tools">
                    <button class="lm-btn" data-act="close" title="Back to the zones">${ICON.back}</button>
                    <button class="lm-btn" data-act="settings" title="Display settings">${ICON.settings}</button>
                    <button class="lm-btn" data-act="info" title="Details">${ICON.info}</button>
                    <button class="lm-btn" data-act="full" title="Full display">${ICON.expand}</button>
                </div>
            </aside>
            <div class="lm-main"><div class="lm-text"></div></div>
            <div class="lm-topline"><div class="lm-fill"></div></div>
            <div class="lm-corner">
                <button class="lm-btn" data-act="settings" title="Display settings">${ICON.settings}</button>
                <button class="lm-btn" data-act="info" title="Details">${ICON.info}</button>
                <button class="lm-btn" data-act="split" title="Show the controls">${ICON.collapse}</button>
            </div>
            <div class="lm-pop lm-settings" hidden></div>
            <div class="lm-pop lm-info" hidden></div>`;
        document.body.appendChild(el);

        const view = LibrettoView.create(el.querySelector('.lm-text'), {
            scroller: el.querySelector('.lm-main'),
            onUpdate: onViewUpdate,
            messages: { noMatch: 'No sung text for this track', noSegments: 'No sung text in this track' },
        });
        lm = { zoneId, full: !!full, album: zone ? zone.album : null, view, el, leaveTimer: null, last: null };

        el.addEventListener('click', onClick);
        el.querySelector('.lm-progress').addEventListener('click', e => {
            const z = currentZone();
            if (z && z.length_seconds && window.handleSeek) window.handleSeek(e, z.zone_id, z.length_seconds);
        });
        // Layout first (mode, text size), so the jump to the current line lands right.
        setFull(lm.full);
        applyScale();
        if (zone) view.resetPosition(zone.position_seconds || 0);
        view.setContent(contentByZone[zoneId] ?? null);
        if (zone) {
            view.setZone(zone);
            renderSide(zone);
        }
        renderSettings();
        checkKeep();
    }

    function close() {
        if (!lm) return;
        clearTimeout(lm.leaveTimer);
        lm.view.destroy();
        lm.el.remove();
        lm = null;
        writeHash();
        decorate();
    }

    function setFull(full) {
        if (!lm) return;
        const changed = lm.full !== !!full;
        lm.full = !!full;
        lm.el.classList.toggle('full', lm.full);
        if (changed) lm.view.recenter();  // the text column changed width
        writeHash();
    }

    // ---- Controls ----
    function onClick(e) {
        const b = e.target.closest('[data-act], [data-opt], [data-size]');
        if (!b || !lm) return;
        const zone = currentZone();
        if (b.dataset.opt) {
            lm.view.setOption(b.dataset.opt, b.dataset.val);
            renderSettings();
            return;
        }
        if (b.dataset.size) {
            scaleIdx = Math.max(0, Math.min(SCALES.length - 1, scaleIdx + parseInt(b.dataset.size, 10)));
            try { localStorage.setItem(SCALE_KEY, String(scaleIdx)); } catch (err) {}
            applyScale();
            renderSettings();
            return;
        }
        switch (b.dataset.act) {
            case 'close': close(); break;
            case 'full': setFull(true); break;
            case 'split': setFull(false); break;
            case 'settings': togglePop('.lm-settings'); break;
            case 'info': togglePop('.lm-info'); break;
            case 'screen': if (window.toggleFullscreen) window.toggleFullscreen(); break;
            case 'previous': case 'next':
                if (zone && window.sendControl) window.sendControl(zone.zone_id, b.dataset.act);
                break;
            case 'playpause':
                if (zone && window.sendControl) window.sendControl(zone.zone_id, zone.state === 'Playing' ? 'pause' : 'play');
                break;
            case 'mute':
                if (zone && window.toggleMute) Promise.resolve(window.toggleMute(zone.zone_id)).then(() => renderSide(currentZone()));
                break;
        }
    }

    function togglePop(sel) {
        const pop = lm.el.querySelector(sel);
        const show = pop.hidden;
        lm.el.querySelectorAll('.lm-pop').forEach(p => { p.hidden = true; });
        pop.hidden = !show;
        if (show && sel === '.lm-info') renderInfo();
    }

    // Tapping outside an open panel closes it; Escape closes a panel, then leaves full.
    document.addEventListener('pointerdown', e => {
        if (!lm || e.target.closest('.lm-pop, [data-act="settings"], [data-act="info"]')) return;
        lm.el.querySelectorAll('.lm-pop').forEach(p => { p.hidden = true; });
    });
    document.addEventListener('keydown', e => {
        if (!lm || e.key !== 'Escape') return;
        const open = [...lm.el.querySelectorAll('.lm-pop')].filter(p => !p.hidden);
        if (open.length) open.forEach(p => { p.hidden = true; });
        else if (lm.full) setFull(false);
    });

    // ---- Text size ----
    // Sized to the window (1 at 1280 x 760, up to 1.8 on large displays), times the
    // reader's own setting.
    function applyScale() {
        if (!lm) return;
        const base = Math.max(1, Math.min(1.8, Math.min(innerWidth / 1280, innerHeight / 760)));
        const zoom = (base * SCALES[scaleIdx]).toFixed(3);
        const text = lm.el.querySelector('.lm-text');
        if (text.style.zoom === zoom) return;
        text.style.zoom = zoom;
        lm.view.recenter();  // line positions moved
    }
    window.addEventListener('resize', applyScale);

    // ---- Drawing ----
    function renderSide(zone) {
        if (!lm || !zone) return;
        const q = sel => lm.el.querySelector(sel);
        const content = contentByZone[lm.zoneId];
        const art = q('.lm-art');
        const src = zone.image_key ? `/image/${encodeURIComponent(zone.image_key)}` : '';
        if (art.getAttribute('src') !== src) {
            if (src) art.setAttribute('src', src); else art.removeAttribute('src');
        }
        art.hidden = !src;
        q('.lm-opera').textContent = (content && content.opera_title) || zone.album || '';
        q('.lm-track').innerHTML = zone.track
            ? (window.formatTrackTitle ? window.formatTrackTitle(zone.track) : escapeHtml(zone.track)) : '';
        q('.lm-artist').textContent = zone.artist || '';
        q('.lm-zone').textContent = zone.zone_name || '';
        const playing = zone.state === 'Playing';
        const pp = q('.lm-pp');
        pp.innerHTML = playing ? ICON.pause : ICON.play;
        pp.classList.toggle('playing', playing);
        const mute = q('.lm-mute');
        mute.innerHTML = zone.is_muted ? ICON.muted : ICON.sound;
        mute.classList.toggle('muted', !!zone.is_muted);
        if (!contentByZone[lm.zoneId]) renderTimes(zone.position_seconds || 0, zone.length_seconds || 0);
    }

    function renderTimes(position, length) {
        if (!lm) return;
        const pct = length > 0 ? Math.min(100, (position / length) * 100) : 0;
        lm.el.querySelectorAll('.lm-fill').forEach(f => { f.style.width = `${pct}%`; });
        lm.el.querySelector('.lm-cur').textContent = formatTime(position);
        lm.el.querySelector('.lm-len').textContent = length ? formatTime(length) : '';
    }

    function onViewUpdate(s) {
        if (!lm) return;
        lm.last = s;
        renderTimes(s.position, s.length);
        const info = lm.el.querySelector('.lm-info');
        if (info && !info.hidden) renderInfo();
    }

    function renderSettings() {
        if (!lm) return;
        const o = lm.view.options;
        const seg = (opt, vals) => vals.map(([v, label]) =>
            `<button class="${o[opt] === v ? 'on' : ''}" data-opt="${opt}" data-val="${v}">${label}</button>`).join('');
        lm.el.querySelector('.lm-settings').innerHTML = `
            <div class="lm-row"><span class="lm-lbl">Translation</span><span class="lm-seg">${seg('translation', [['aligned', 'Aligned'], ['line', 'Line'], ['off', 'Off']])}</span></div>
            <div class="lm-row"><span class="lm-lbl">Ensemble</span><span class="lm-seg">${seg('ensemble', [['stanza', 'Stanza'], ['rows', 'Rows']])}</span></div>
            <div class="lm-row"><span class="lm-lbl">Bars</span><span class="lm-seg">${seg('bars', [['on', 'On'], ['off', 'Off']])}</span></div>
            <div class="lm-row"><span class="lm-lbl">Text size</span><span class="lm-seg">
                <button data-size="-1" ${scaleIdx === 0 ? 'disabled' : ''}>A−</button>
                <span class="lm-size">${Math.round(SCALES[scaleIdx] * 100)}%</span>
                <button data-size="1" ${scaleIdx === SCALES.length - 1 ? 'disabled' : ''}>A+</button></span></div>
            <div class="lm-row"><span class="lm-lbl">Screen</span><span class="lm-seg"><button data-act="screen">Browser full screen</button></span></div>`;
    }

    // Details for checking the match and the timing (what the old page's header showed).
    function renderInfo() {
        if (!lm) return;
        const info = lm.el.querySelector('.lm-info');
        if (info.hidden) return;
        const zone = currentZone();
        const c = contentByZone[lm.zoneId];
        const s = lm.last || lm.view.state();
        const row = (k, v) => v ? `<tr><th>${k}</th><td>${v}</td></tr>` : '';
        let now = '';
        if (s.inEnsemble) now = 'ensemble card';
        else if (s.segment) {
            const seg = s.segment;
            now = `line ${s.segmentIndex + 1} of ${s.segmentCount}, ${formatTime(seg.start)}` +
                  (s.nextSegment ? ` → ${formatTime(s.nextSegment.start)}` : '');
            const where = [seg.character, seg.piece,
                seg.bar_start != null ? `bars ${+seg.bar_start.toFixed(1)}–${+(seg.bar_end ?? seg.bar_start).toFixed(1)}` : '']
                .filter(Boolean).map(escapeHtml).join(' · ');
            if (where) now += `<br>${where}`;
        }
        info.innerHTML = `<table>
            ${row('Match', c ? `${escapeHtml(String(c.match_method || ''))} · ${escapeHtml(c.file_id)} · track ${c.track_index}` : 'none')}
            ${row('Roon track', zone && zone.track ? escapeHtml(zone.track) : '')}
            ${row('File track', c ? escapeHtml(c.track_title || '') + (c.track_id ? ` (${escapeHtml(c.track_id)})` : '') : '')}
            ${row('Roon album', zone && zone.album ? escapeHtml(zone.album) : '')}
            ${row('Libretto', c ? escapeHtml([c.opera_title, c.artist].filter(Boolean).join(' · ')) : '')}
            ${row('Lines', c ? String(s.segmentCount) : '')}
            ${row('Now', now)}
            ${row('Position', `${formatTime(s.position)} (${s.position.toFixed(1)} s) of ${formatTime(s.length)} · lead ${LibrettoView.LEAD_SECONDS} s`)}
            ${row('Zone', zone ? escapeHtml(`${zone.zone_name} · ${zone.state}`) : '')}
            ${row('Leaving', lm.leaveTimer ? 'no libretto and a different album: back to the zones shortly' : '')}
        </table>
        <a class="lm-link" href="/libretto-view" target="_blank" rel="noopener">Open the full libretto page</a>`;
    }

    window.LibrettoMode = { decorate, onZones, onLibrettoUpdate, onSeek, open, close };
})();
