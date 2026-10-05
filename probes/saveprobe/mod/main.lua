#version 2
-- Save probe (a test mod, not for the Workshop): how fast do savegame.mod keys reach
-- %LOCALAPPDATA%\Teardown\savegame.xml? A voice-chat sidecar would read the game's state from there.
-- Run probes/saveprobe/watch.py first, then play any level with this mod enabled for ~1 minute.
--   wait 3 s, then
--   A  20 s: every frame write savegame.mod.svp.{pay, t, ph, n} (n = frame counter, written last)
--   B  20 s: the same at 10 Hz (the rate a sidecar feed would use)
--   C  10 s: no writes (does the file still change?)
--   done: ph = "done", and 3 s later the keys are cleared
-- In the same frame as a write it marks the log: Spawn("<body tags='svp n=.. t=..'/>") is logged with a
-- wall-clock time, so the watcher can tell how late each write reached the disk.
-- Everything runs in server.tick (single player: same machine, same registry, same frame).

local WAIT, A, B, C = 3, 20, 20, 10

local function mark(n, tms, ph)
	local ok, hs = pcall(Spawn, "<body tags='svp n=" .. n .. " t=" .. tms .. " ph=" .. ph .. "'/>",
		Transform(Vec(0, -500, 0)), true)
	if ok and hs then
		for i = 1, #hs do Delete(hs[i]) end
	end
end

local function payload(n)
	-- the size of a real feed: 8 peers x "id,gain,azimuth,muffle;"
	local t = {}
	for i = 1, 8 do
		t[i] = string.format("%d,%.3f,%.1f,%.2f", i, ((n * 7 + i) % 1000) / 1000, (n * 3 + i * 40) % 360,
			((n + i) % 100) / 100)
	end
	return table.concat(t, ";")
end

local function write(n, tms, ph)
	SetString("savegame.mod.svp.pay", payload(n))
	SetInt("savegame.mod.svp.t", tms)
	SetString("savegame.mod.svp.ph", ph)
	SetInt("savegame.mod.svp.n", n)
end

function server.init()
	server.t0 = nil
	server.n = 0
	server.lastB = -1
	server.cleared = false
	server.phase = ""
	ClearKey("savegame.mod.svp")
end

function server.tick(dt)
	local now = GetTime()
	if not server.t0 then server.t0 = now end
	local e = now - server.t0
	server.n = server.n + 1
	local n, tms = server.n, math.floor(now * 1000 + 0.5)
	local ph, left
	if e < WAIT then
		ph, left = "wait", WAIT - e
	elseif e < WAIT + A then
		ph, left = "a", WAIT + A - e
	elseif e < WAIT + A + B then
		ph, left = "b", WAIT + A + B - e
	elseif e < WAIT + A + B + C then
		ph, left = "c", WAIT + A + B + C - e
	else
		ph, left = "done", 0
	end
	local entered = ph ~= server.phase
	server.phase = ph

	if ph == "a" then
		write(n, tms, ph)
		if entered or n % 15 == 0 then mark(n, tms, ph) end
	elseif ph == "b" then
		local slot = math.floor((e - WAIT - A) * 10)
		if slot ~= server.lastB then
			server.lastB = slot
			write(n, tms, ph)
			mark(n, tms, ph)
		end
	elseif entered and (ph == "c" or ph == "done") then
		write(n, tms, ph)
		mark(n, tms, ph)
	elseif ph == "done" and not server.cleared and e > WAIT + A + B + C + 3 then
		ClearKey("savegame.mod.svp")
		server.cleared = true
	end
	SetString("svp.phase", ph)
	SetFloat("svp.left", left)
end

function client.draw()
	local ph = GetString("svp.phase")
	local names = {wait = "starting", a = "A: every frame", b = "B: 10 Hz", c = "C: no writes", done = "DONE"}
	local line = "SAVE PROBE   " .. (names[ph] or "?")
	if ph ~= "done" then
		line = line .. string.format("   %.0f s", GetFloat("svp.left"))
	else
		line = line .. " - you can quit; the watcher prints its report"
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
