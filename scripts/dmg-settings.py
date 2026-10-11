# Layout of Kuvo.dmg, read by dmgbuild (see scripts/build-dmg.sh): the app next
# to a link to Applications, with an arrow between them, so installing is one drag.
import os.path

application = defines.get("app", "build/Kuvo.app")  # noqa: F821 (provided by dmgbuild)
name = os.path.basename(application)

files = [application]
symlinks = {"Applications": "/Applications"}
icon = "Resources/Icon/AppIcon.icns"

format = "ULFO"  # LZFSE compression; macOS 10.11 and later
filesystem = "APFS"

background = "builtin-arrow"
window_rect = ((200, 120), (600, 360))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False

icon_size = 112
text_size = 13
icon_locations = {
    name: (150, 165),
    "Applications": (450, 165),
}
