# Fixtures

## `fragmented.m4a`

Un MP4 fragmentado (fMP4): `ftyp`, `moov` con `mvex`, y después pares `moof`/`mdat`. Es el
formato que manda YouTube para el audio (itag 140, AAC-LC).

Se generó con ffmpeg (un tono de 440 Hz, sin derechos de autor):

```text
ffmpeg -f lavfi -i sine=frequency=440:duration=2 -c:a aac -b:a 64k \
    -movflags frag_keyframe+empty_moov+default_base_moof fragmented.m4a
```

`empty_moov` deja la duración en cero, así que después se corrigieron los campos de duración
de `mvhd` y `mdhd` a 2 s (88200 muestras a 44100 Hz). Nada más se tocó.
