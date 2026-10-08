"""The games Koetama works with. Each is a module with a class derived from base.Game; the app lists GAMES in its
game picker. A new game: a new module here (its link: how the game tells Koetama whom the player hears and asks
for the microphone, and how Koetama hands the game what the player said), added to GAMES.
"""
from .teardown import Teardown

GAMES = [Teardown]


def by_id(game_id):
    for g in GAMES:
        if g.id == game_id:
            return g
    return GAMES[0]
