## The Godot client against a running Koetama, headless (no game, no window):
##     koetama.exe --cli --game godot-example --type ...          (lines typed into it come back as speech)
##     koetama.exe --cli --game godot-example --mic-wav x.wav ... (a recording as the microphone: speech, and the voice)
##     godot --headless --script test_koetama.gd -- <seconds>
## Prints each object as it arrives, asks for es -> en and one line translated, and says how long each took. The
## feed also names a voice room (a room_seed made from the time), this player's string id, where they hear from, a
## range, and one other player by position: the voice chat connects, and with --mic-wav this player's own voice going
## out comes back as talking (id = me).
extends SceneTree

var k: Node
var t0 := 0.0
var until := 30.0
var asked := false


func _initialize() -> void:
	var args := OS.get_cmdline_user_args()
	if args.size() > 0:
		until = float(args[0])
	k = load("res://koetama.gd").new()
	k.port = 47150
	k.feed.listen = "always"
	k.feed.me = "godot-tester"
	k.feed.name = "Tester"
	k.feed.room_seed = "koetama godot test %d" % int(Time.get_unix_time_from_system())
	k.feed.range = [8, 25]
	# (Godot's camera looks down -z)
	k.feed.listener = {"position": [0, 1.7, 0], "forward": [0, 0, -1], "right": [1, 0, 0], "up": [0, 1, 0]}
	k.feed.speakers = [{"id": "godot-friend", "name": "Friend", "position": [2, 1.7, -3]}]
	k.hello.connect(func(f): _say("hello, features %s" % [f]))
	k.status.connect(func(speech, mic): _say("status: speech %s, microphone %s" % [speech, mic]))
	k.voice.connect(func(state, players): _say("voice %s, players %s" % [state, players]))
	k.talking.connect(func(id, on): _say("talking %s %s (now: %s)" % [id, on, k.talking_now.keys()]))
	k.speech.connect(func(kind, utt, text, _o): _say("speech %s %d: %s" % [kind, utt, text]))
	k.translation.connect(func(id, text, from, to): _say("translation %d (%s -> %s): %s" % [id, from, to, text]))
	k.translations_status.connect(func(ts): _say("translations %s" % [ts]))
	root.add_child(k)
	t0 = Time.get_ticks_msec() / 1000.0


func _say(s: String) -> void:
	print("%6.2f s  %s" % [Time.get_ticks_msec() / 1000.0 - t0, s])


func _process(_delta: float) -> bool:
	var now := Time.get_ticks_msec() / 1000.0 - t0
	if k.connected() and not asked:
		asked = true
		k.feed.translations = [{"from": "es", "to": "en"}]
	# (the line to translate, until its translation is back - Koetama answers "" while the models are not ready)
	k.feed.to_translate = [{"id": 1 + int(now / 5.0), "text": "¿Dónde está la llave del sótano?"}] if asked else []
	return now > until
