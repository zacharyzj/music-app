local input = require "mp.input"
local utils = require "mp.utils"
local assdraw = require "mp.assdraw"

local speeds = {0.5, 0.75, 1, 1.25, 1.5, 1.75, 2}
local function video_menu(prompt, items, selected, apply)
    local entry = mp.get_property("playlist/0/id")
    if not entry or not mp.get_property_native("vo-configured") then return end
    mp.commandv("script-message-to", "osc", "osc-hide")
    input.select({
        prompt = prompt, items = items, default_item = selected,
        submit = function(index)
            if mp.get_property("playlist/0/id") == entry and mp.get_property_native("vo-configured") then
                apply(index)
            end
        end,
    })
end

local function select_speed()
    local items, selected = {}, 1
    for i, speed in ipairs(speeds) do
        items[i] = speed .. "×"
        if speed == mp.get_property_number("speed") then selected = i end
    end
    video_menu("播放倍速", items, selected, function(i)
        local speed = speeds[i]
        if mp.set_property_number("speed", speed) then
            mp.commandv("script-message", "music-video", "speed", tostring(speed))
            mp.osd_message(items[i])
        end
    end)
end

local function select_quality()
    local video = utils.parse_json(mp.get_property_native("user-data/music-video", ""))
    if not video or video.path ~= mp.get_property("path") then return end
    if #video.qualities == 0 then
        mp.osd_message("原始画质 · 当前视频仅提供一个清晰度")
        return
    end
    local items, selected = {"自动"}, 1
    local current = mp.get_property("current-tracks/video/external-filename", video.path)
    for i, q in ipairs(video.qualities) do
        items[i + 1] = q.height .. "p"
        if q.url == current then items[i + 1] = items[i + 1] .. "  ·  播放中" end
        if video.preferred == q.height then selected = i + 1 end
    end
    video_menu("视频清晰度", items, selected, function(i)
        local q = video.qualities[i - 1] or {height = 0, url = video.auto}
        local err
        if q.url ~= current then
            if q.url == video.path then
                for _, track in ipairs(mp.get_property_native("track-list", {})) do
                    if track.type == "video" and not track.external then
                        _, err = mp.set_property_number("vid", track.id)
                        break
                    end
                end
            else
                -- Change only the picture track. mpv keeps the audio clock, pause state and speed.
                local _, reason = mp.command_native({"video-add", q.url, "cached", q.height .. "p"})
                err = reason
            end
        end
        if err then mp.osd_message("画质切换失败"); return end
        if video.path ~= mp.get_property("path") or not mp.get_property_native("vo-configured") then return end
        video.preferred = q.height
        mp.set_property_native("user-data/music-video", utils.format_json(video))
        mp.commandv("script-message", "music-video", "height", tostring(q.height))
        mp.osd_message(q.height == 0 and "自动画质" or q.height .. "p")
    end)
end

local buttons = {
    {key = "speed", label = "倍速", select = select_speed},
    {key = "quality", label = "画质", select = select_quality},
}
local overlay = mp.create_osd_overlay("ass-events")
overlay.z = 1001
local visible, hovered = false, nil
local button_width, button_height, button_gap = 66, 26, 9
local bar, bar_visible = nil, false

local function contains(b, x, y)
    return x >= b.x1 and x < b.x2 and y >= b.y1 and y < b.y2
end

local function draw()
    local ass = assdraw.ass_new()
    for _, b in ipairs(buttons) do
        local active = hovered == b
        local fill = active and "F2C2D8" or "382B2D"
        local border = active and "FCE4EF" or "8F7581"
        local ink = active and "361E29" or "FFF8FA"
        ass:new_event()
        ass:pos(b.left, b.top)
        ass:append(("{\\an7\\shad0\\bord1\\1c&H%s&\\3c&H%s&}"):format(fill, border))
        ass:draw_start()
        ass:round_rect_cw(1, 1, button_width - 1, button_height - 1, 7)
        ass:draw_stop()
        ass:new_event()
        ass:pos(b.left + (button_width - 14) / 2, b.top + button_height / 2)
        ass:append(("{\\an5\\fs15\\b600\\bord0\\shad0\\1c&H%s&}%s"):format(ink, b.label))
        ass:new_event()
        ass:pos(b.left + button_width - 18, b.top + button_height / 2 - 2)
        ass:append(("{\\an7\\bord0\\shad0\\1c&H%s&}"):format(ink))
        ass:draw_start()
        ass:move_to(0, 0)
        ass:line_to(5, 4)
        ass:line_to(10, 0)
        ass:line_to(10, 2)
        ass:line_to(5, 6)
        ass:line_to(0, 2)
        ass:draw_stop()
    end
    overlay.data = ass.text
    overlay:update()
end

-- A user can leave the pointer on a button while deciding which menu to open.
local keep_visible = mp.add_periodic_timer(1, function()
    mp.commandv("script-message-to", "osc", "osc-show")
end)
keep_visible:kill()

local function set_hover(button)
    if button == hovered then return end
    hovered = button
    if button then
        mp.commandv("script-message-to", "osc", "osc-show")
        keep_visible:resume()
    else
        keep_visible:kill()
    end
    if visible then draw() end
end

local function layout()
    local dims = mp.get_property_native("osd-dimensions")
    if not bar_visible or not bar or not dims or dims.w <= 0 or dims.h <= 0
        or not mp.get_property_native("vo-configured")
        or mp.get_property_native("user-data/mpv/console/open") then
        if visible then
            visible, hovered = false, nil
            keep_visible:kill()
            for _, b in ipairs(buttons) do mp.disable_key_bindings(b.section) end
            overlay:remove()
        end
        return
    end
    local dpi = math.max(1, mp.get_property_number("display-hidpi-scale", 1))
    local scale = math.min(dpi, dims.w / 240, dims.h / 150)
    overlay.res_x = math.floor(dims.w / scale + 0.5)
    overlay.res_y = math.floor(dims.h / scale + 0.5)
    local sx, sy = dims.w / overlay.res_x, dims.h / overlay.res_y
    -- The 90-unit floating bar has playlist actions at its left and transport in the middle.
    local unit = (bar.y2 - bar.y1) / (90 * sy)
    local left = bar.x1 / sx + 97 * unit
    local transport = (bar.x1 + bar.x2) / (2 * sx) - 95 * unit
    button_width = math.min(66, math.floor((transport - left - button_gap) / 2))
    local top = bar.y2 / sy - 20 * unit - button_height / 2
    if button_width < 52 or unit < 0.85 then
        button_width = 68
        left = bar.x2 / sx - 2 * button_width - button_gap - 8
        top = math.max(0, bar.y1 / sy - button_height - 5)
    end
    top = math.floor(top + 0.5)
    for i, b in ipairs(buttons) do
        b.left = math.floor(left + (i - 1) * (button_width + button_gap))
        b.top = top
        b.x1, b.y1 = math.floor(b.left * sx + 0.5), math.floor(top * sy + 0.5)
        b.x2, b.y2 = math.floor((b.left + button_width) * sx + 0.5), math.floor((top + button_height) * sy + 0.5)
        mp.set_mouse_area(b.x1, b.y1, b.x2, b.y2, b.section)
        if not visible then mp.enable_key_bindings(b.section) end
    end
    local mouse = mp.get_property_native("mouse-pos")
    local under_pointer
    if mouse and mouse.hover then
        for _, b in ipairs(buttons) do
            if contains(b, mouse.x, mouse.y) then under_pointer = b end
        end
    end
    set_hover(under_pointer)
    visible = true
    draw()
end

for _, button in ipairs(buttons) do
    button.section = "music-video-" .. button.key
    mp.add_key_binding(nil, button.key, button.select)
    mp.set_key_bindings({
        {"mouse_move", function() if visible then set_hover(button) end end},
        {"mouse_leave", function() if hovered == button then set_hover(nil) end end},
        {"MBTN_LEFT", function()
            if visible and contains(button, mp.get_mouse_pos()) then button.select() end
        end},
    }, button.section, "force")
end
for _, property in ipairs({"vo-configured", "osd-dimensions", "display-hidpi-scale",
                          "user-data/mpv/console/open"}) do
    mp.observe_property(property, "native", layout)
end

-- Attach to the bundled OSC's input section so the buttons share its actual
-- position and visibility, even when the video changes size or enters fullscreen.
local set_area, enable, disable = mp.set_mouse_area, mp.enable_key_bindings, mp.disable_key_bindings
function mp.set_mouse_area(x1, y1, x2, y2, section)
    set_area(x1, y1, x2, y2, section)
    if section == "input" and (not bar or bar.x1 ~= x1 or bar.y1 ~= y1 or bar.x2 ~= x2 or bar.y2 ~= y2) then
        bar = {x1 = x1, y1 = y1, x2 = x2, y2 = y2}
        layout()
    end
end
function mp.enable_key_bindings(section, flags)
    enable(section, flags)
    if section == "input" then
        bar_visible = true
        layout()
    end
end
function mp.disable_key_bindings(section)
    disable(section)
    if section == "input" then
        bar_visible = false
        layout()
    end
end
require "@osc.lua"
