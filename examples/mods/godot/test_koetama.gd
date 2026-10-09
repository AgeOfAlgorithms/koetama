## The Godot client against a running Koetama, headless (no game, no window):
##     koetama.exe --cli --game godot-example --type ...      (lines typed into it come back as speech)
##     godot --headless --script test_koetama.gd -- <seconds>
## Prints each object as it arrives, asks for es -> en and one line translated, and says how long each took.
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
	k.feed.session = 1
	k.hello.connect(func(f): _say("hello, features %s" % [f]))
	k.speech.connect(func(kind, utt, text, _o): _say("speech %s %d: %s" % [kind, utt, text]))
	k.translation.connect(func(id, text): _say("translation %d: %s" % [id, text]))
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
