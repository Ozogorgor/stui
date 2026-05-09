-- stui-overlay.lua — minimal status overlay owned by stui.
--
-- Replaces mpv's default idle screen ("Drop files or URLs here to play")
-- and the small top-left `show-text` toast with one centered overlay
-- driven by the stui runtime over IPC.
--
-- ## API (from runtime → mpv → here)
--
-- `script-message stui-status <text>`  — show <text> centered. Empty clears.
-- `script-message stui-busy <on|off>`  — animate a spinner prefix.
--
-- ## Auto-behaviour
--
-- The overlay self-clears on `playback-restart` so a stale
-- "Fetching torrent metadata…" can't bleed through once frames flow.

local mp = require 'mp'

local state = {
    status      = nil,
    busy        = false,
    spinner_idx = 1,
    timer       = nil,
}

-- Quarter-circle spinner — Unicode geometric block (U+25D0-3) is widely
-- present in default fonts where Braille (U+2800-range) is often
-- substituted (mpv-osd-symbols rendered the Braille glyphs as a music
-- note). Four frames give a clean rotation feel.
local SPINNER = {"◐","◓","◑","◒"}

local overlay = mp.create_osd_overlay("ass-events")

local function render()
    if not state.status or state.status == "" then
        overlay.data = ""
        overlay:update()
        return
    end

    local osd_w, osd_h = mp.get_osd_size()
    if not osd_w or osd_w == 0 then return end

    -- Anchor the ASS coordinate system to the actual OSD pixel
    -- dimensions. Without this, ass-events overlays use mpv's default
    -- script-resolution (~1280×720) regardless of the real window
    -- size, so `\pos(osd_w/2, osd_h/2)` lands wherever (osd_w/2,
    -- osd_h/2) happens to fall in mpv's coordinate space — which is
    -- *not* dead-centre once the window is bigger than 720p.
    overlay.res_x = osd_w
    overlay.res_y = osd_h

    local prefix = state.busy and (SPINNER[state.spinner_idx] .. "  ") or ""
    local text = prefix .. state.status

    local cx = math.floor(osd_w / 2)
    local cy = math.floor(osd_h / 2)

    -- \fnSans forces a sans-serif fallback so we don't render in
    -- mpv-osd-symbols (the default OSD font lacks lots of Unicode
    -- ranges and substitutes weirdly — Braille came out as ♪).
    overlay.data = string.format(
        "{\\an5\\pos(%d,%d)\\fnSans\\fs28\\bord3\\3c&H000000&\\1c&HFFFFFF&}%s",
        cx, cy, text
    )
    overlay:update()
end

local function ensure_timer()
    if state.busy and not state.timer then
        state.timer = mp.add_periodic_timer(0.1, function()
            state.spinner_idx = (state.spinner_idx % #SPINNER) + 1
            render()
        end)
    elseif not state.busy and state.timer then
        state.timer:kill()
        state.timer = nil
    end
end

mp.register_script_message("stui-status", function(text)
    state.status = text
    render()
end)

mp.register_script_message("stui-busy", function(flag)
    state.busy = (flag == "1" or flag == "on" or flag == "true")
    ensure_timer()
    render()
end)

-- Once a frame is actually showing, the runtime's "fetching…" message
-- is by definition stale. Clear and stop the spinner.
mp.register_event("playback-restart", function()
    state.status = nil
    state.busy   = false
    ensure_timer()
    render()
end)

-- Re-render on resize so the centring stays correct.
mp.observe_property("osd-dimensions", "native", render)
