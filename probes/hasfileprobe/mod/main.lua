#version 2
#include "abs.lua"
-- HasFile probe (a test mod, not for the Workshop): can a helper program signal the game by creating
-- and deleting files? HasFile is the only thing a script can ask the disk. Run probes/hasfileprobe/probe.py
-- first (it installs this mod and toggles the files), then play any level with this mod enabled, ~50 s.
-- Every frame (client side: in real use each machine asks its own disk) it writes to savegame.mod.hfp:
--   s   one character per file below: 1 there, 0 not there, e the call failed
--   ph  base (25 s) / heavy (10 s: 60 more HasFile calls a frame, for the cost) / sound (14 s) / done
--   n   frame counter (written last)
-- Sound phase: a control beep that shipped with the mod (1 beep), then two files the probe dropped into
-- the mod folder AFTER the level loaded (2 beeps, 3 beeps): does LoadSound read a file that is new?
--   h0 h1 h2  the LoadSound handles, f1 f2  HasFile for the two new files when they were loaded

local BASE, HEAVY, SOUND = 25, 10, 14
local FILES = {"MOD/sig/pre.txt", "MOD/sig/a.txt", "MOD/sig/b", "MOD/../hfp_up.txt"}
local SOUNDS = {{1.0, "MOD/snd/control.ogg", "h0"}, {5.0, "MOD/sig/live1.ogg", "h1", "f1"}, {9.0, "MOD/sig/live2.ogg", "h2", "f2"}}

local function has(path)
	local ok, v = pcall(HasFile, path)
	if not ok then return "e" end
	return v and "1" or "0"
end

function client.init()
	client.t0 = nil
	client.n = 0
	client.played = {}
	client.phase = "base"
	client.left = 0
	client.note = ""
	if HFP_ABS ~= "" then FILES[#FILES + 1] = HFP_ABS end
	ClearKey("savegame.mod.hfp")
end

function client.tick(dt)
	local now = GetTime()
	if not client.t0 then client.t0 = now end
	local e = now - client.t0
	client.n = client.n + 1
	local ph
	if e < BASE then
		ph, client.left = "base", BASE - e
	elseif e < BASE + HEAVY then
		ph, client.left = "heavy", BASE + HEAVY - e
	elseif e < BASE + HEAVY + SOUND then
		ph, client.left = "sound", BASE + HEAVY + SOUND - e
	else
		ph, client.left = "done", 0
	end
	client.phase = ph

	local s = {}
	for i = 1, #FILES do s[i] = has(FILES[i]) end
	if ph == "heavy" then
		for i = 1, 60 do has("MOD/sig/none" .. i) end
	end
	if ph == "sound" then
		local se = e - BASE - HEAVY
		for i, d in ipairs(SOUNDS) do
			if se >= d[1] and not client.played[i] then
				client.played[i] = true
				if d[4] then SetString("savegame.mod.hfp." .. d[4], has(d[2])) end
				local ok, h = pcall(LoadSound, d[2])
				SetString("savegame.mod.hfp." .. d[3], ok and tostring(h) or "error")
				if ok and h then pcall(PlaySound, h, GetCameraTransform().pos, 1.0, false) end
				client.note = i == 1 and "now: 1 beep (control, shipped with the mod)"
					or ("now: " .. i .. " beeps (file " .. (i - 1) .. ", new since the level loaded)")
			end
		end
	end
	SetString("savegame.mod.hfp.s", table.concat(s))
	SetString("savegame.mod.hfp.ph", ph)
	SetInt("savegame.mod.hfp.n", client.n)
end

function client.draw()
	local ph = client.phase or "base"
	local names = {base = "files", heavy = "cost", sound = "SOUND - listen: " .. (client.note or ""), done = "DONE"}
	local line = "HASFILE PROBE   " .. (names[ph] or "?")
	if ph ~= "done" then
		line = line .. string.format("   %.0f s", client.left or 0)
	else
		line = line .. " - which beeps did you hear (1, 2, 3)? You can quit."
	end
	UiPush()
	UiTranslate(UiCenter(), 40)
	UiAlign("center top")
	UiFont("bold.ttf", 24)
	local w = UiGetTextSize(line)
	UiColor(0, 0, 0, 0.6)
	UiRect(w + 40, 44)
	UiTranslate(0, 8)
	UiColor(1, 0.9, 0.4)
	UiText(line)
	UiPop()
end
