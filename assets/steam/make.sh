#!/bin/sh
# Regenerate the gamestore tile's Steam artwork with ImageMagick alone:
# portrait (600x900), wide (920x430), hero (1920x620), logo and icon.
set -eu
cd "$(dirname "$0")"
# A static file: ImageMagick cannot pick a weight out of a variable font.
FONT=$(fc-match -f '%{file}' 'Fira Sans:style=ExtraBold')
BG1='#15111f'; BG2='#3a1f63'; ACCENT='#be8cff'; DIM='#9a90b0'

# A faint wall of rounded "covers", the grid the UI shows.
tiles() { # width height tile
    w=$1; h=$2; t=$3; draw=""
    y=$((-t / 2)); row=0
    while [ $y -lt $h ]; do
        x=$(( (row % 2) * t / 2 - t / 2 ))
        while [ $x -lt $w ]; do
            draw="$draw roundrectangle $x,$y $((x + t * 7 / 10)),$((y + t)) $((t / 12)),$((t / 12))"
            x=$((x + t * 8 / 10))
        done
        y=$((y + t + t / 8)); row=$((row + 1))
    done
    printf '%s' "$draw"
}

background() { # width height tile out
    magick -size "$1x$2" gradient:"$BG1-$BG2" -rotate 0 \
        \( -size "$1x$2" xc:none -fill 'rgba(190,140,255,0.07)' -draw "$(tiles "$1" "$2" "$3")" \) \
        -composite "$4"
}

wordmark() { # pointsize out (transparent)
    magick -background none -font "$FONT" -pointsize "$1" \
        \( -fill white label:Game \) \( -fill "$ACCENT" label:Store \) +append \
        -trim +repage "$2"
}

wordmark 220 logo.png
magick logo.png -resize 1600x -background none -gravity center -extent 1700x500 logo.png

# The portrait is what the library grid shows most: the two words stacked, large.
background 600 900 150 p.png
magick -background none -font "$FONT" -pointsize 150 \
    \( -fill white label:Game \) \( -fill "$ACCENT" label:Store \) -gravity center -append \
    -trim +repage stacked.png
magick p.png \( stacked.png -resize 470x \) -gravity center -geometry +0-50 -composite \
    -font "$FONT" -pointsize 44 -fill "$DIM" -gravity center -annotate +0+190 'GOG · Steam' p.png
wordmark 120 wm.png

background 920 430 110 wide.png
magick wide.png \( wm.png -resize 640x \) -gravity center -geometry +0-25 -composite \
    -font "$FONT" -pointsize 36 -fill "$DIM" -gravity center -annotate +0+70 'GOG · Steam' wide.png

background 1920 620 160 hero.png

magick -size 256x256 xc:none -fill "$BG2" -draw 'roundrectangle 8,8 247,247 48,48' \
    -font "$FONT" -pointsize 170 -fill "$ACCENT" -gravity center -annotate +0+6 'G' icon.png

rm -f wm.png stacked.png
