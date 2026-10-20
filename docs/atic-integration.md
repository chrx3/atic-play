# Atic-play + Atic: plan de integración

Objetivo: que las dos apps funcionen perfecto cuando ambas están instaladas, sin
depender una de la otra cuando solo hay una. Atic-play es el reproductor; la pill
de Atic es el widget (su panel **Ahora suena**). Atic-play ya no tiene mini
reproductor propio.

Repos:

- **Atic-play**: este repositorio (Rust + GPUI).
- **Atic**: [chrx3/atic](https://github.com/chrx3/atic).
  La pill nativa está en `prototypes/pill-gpui` (GPUI, solo Windows); su Ahora
  suena es `src/media.rs`, y las letras están en `src/media/lyrics.rs`.

Regla de diseño: **cada fase debe funcionar sola**. Si solo está Atic-play, toca
como siempre. Si solo está Atic, su Ahora suena sigue leyendo cualquier app. Si
están las dos, se reconocen y suman funciones.

---

## 1. Lo que ya funciona hoy (sin tocar código)

Atic-play publica lo que suena en la sesión de medios de Windows (SMTC)
mediante `souvlaki`, desde un hilo propio (`crates/desktop/src/media_session.rs`).
Atic lee esa misma sesión con `GlobalSystemMediaTransportControlsSessionManager`
(`media.rs`), así que **hoy ya muestra Atic-play** en Ahora suena con:

| Dato o acción | Atic-play lo publica | Atic lo usa |
|---|---|---|
| Título, artista, álbum | `set_metadata` | Sí |
| Carátula | `cover_url` = `file://` del caché de portadas | Sí (carátula, fondo y portada) |
| Duración y posición | `duration` + `MediaPosition` | Sí (barra que se arrastra) |
| Play/pausa, siguiente, anterior | Recibe `MediaControlEvent` | Sí |
| Ir a un segundo, ±10 s | `SetPosition` / `Seek` | Sí |
| Volumen de la app | Sesión de audio normal (rodio/WASAPI) | Sí (mezclador por app) |
| Onda de la pill | Atic mide la salida real (`IAudioMeterInformation`) | Sí |
| Letra sincronizada | Atic-play la baja de LRCLIB para su panel | Atic la baja de LRCLIB por su cuenta |

## 2. Problemas a corregir antes de nada

### 2.1 Cambiar la salida desde Atic pausa la música ✅ (resuelto en Atic-play)

- Atic cambia la salida de audio por omisión del sistema
  (`IPolicyConfig::SetDefaultEndpoint`, `src/system/os.rs`).
- Desde `8f4152d`, Atic-play **se pausa siempre que cambia el dispositivo
  predeterminado**. Así los audífonos desconectados no siguen sonando por los
  parlantes (`follow_audio_device` en `crates/desktop/src/playback.rs`).
- Resultado: elegir otra salida en el panel de Atic pausa Atic-play. Hay que
  darle play de nuevo.

**Arreglo (hecho en Atic-play, no depende de Atic):** distinguir entre "se
desconectó" y "se eligió otra salida". `AudioPlayer` guarda el id del dispositivo
que abrió y `device_still_connected()` lo busca entre las salidas activas.

- Al perder el stream, mirar si el dispositivo anterior **sigue en la lista de
  salidas activas** (`cpal::default_host().output_devices()`, comparando el
  nombre o id que se guardó al abrirlo en `core/src/player.rs`).
- **Si ya no está** (se desconectó): pasar al nuevo dispositivo **en pausa**,
  como hoy.
- **Si sigue ahí** (se cambió a mano, desde Windows o desde Atic): pasar al
  nuevo dispositivo y **seguir sonando**.

Esto también mejora el cambio manual desde la configuración de Windows. Si la
fase 3 existe, Atic puede además avisar "cambio intencional", aunque no debería
hacer falta.

### 2.2 El nombre de la fuente sale como "atic-play"

- Atic-play no está empaquetado, así que su sesión SMTC se presenta con el
  AppUserModelID que Windows deriva del exe.
- `media::source_name` lo convierte en `atic-play` (en minúsculas y sin ícono
  propio).

**Arreglo:**

- **Atic-play:** fijar un AUMID explícito al iniciar, antes de abrir la ventana
  (`SetCurrentProcessExplicitAppUserModelID("Ciat.AticPlay")`). Va en `main.rs`,
  con FFI a `shell32` o con el crate `windows`.
  - Verificar que la barra de tareas siga agrupando bien y que los anclajes
    existentes no se rompan; el instalador debería usar el mismo id en su
    acceso directo.
- **Atic:** agregar `("aticplay", "Atic-play")` a la tabla `known` de
  `source_name` y `same_app`, y usar el ícono de Atic-play para esa fuente.

### 2.3 Dos mini reproductores

Resuelto: el mini reproductor de Atic-play se quitó (`Remove the mini player`).
La pill de Atic es el único widget.

## 3. Lo que SMTC no puede dar

SMTC solo transporta metadatos básicos y controles de transporte. Lo que Atic
no puede pedirle a Atic-play por esa vía:

| Función | ¿Por SMTC? | Valor en la pill |
|---|---|---|
| Me gusta / quitar me gusta (y su estado) | No | Alto: un corazón en Ahora suena |
| Guardar en playlist | No | Medio: abrir un selector |
| Ver la cola y saltar a una canción | No | Medio: lista de "A continuación" |
| Aleatorio y repetir | SMTC los define, pero souvlaki no los expone | Medio |
| Letra ya resuelta (la misma que muestra Atic-play) | No | Bajo: Atic ya la baja él mismo; evita una segunda consulta y que las dos muestren versiones distintas |
| Abrir Atic-play en la canción, el álbum o el artista | No; solo `Raise`, que ya trae la ventana | Medio |
| "Radio" desde lo que suena | No | Bajo |
| Estado: cargando, sin conexión, sin salida de audio | No | Medio: la pill puede mostrar por qué no suena |

Mientras nada de esto haga falta, **la fase 1 (SMTC) es suficiente** y no se
agrega ningún canal propio.

## 4. Canal propio (solo si se quieren las funciones de la sección 3)

**Atic-play es el servidor, Atic el cliente.** Atic-play es dueño del estado
(cola, me gusta, sesión de YouTube Music). Atic solo lo muestra y manda
órdenes. Si el canal no está, Atic sigue con SMTC y nada se rompe.

### Transporte

- Named pipe local: `\\.\pipe\atic-play-<usuario>`.
  - Lo crea Atic-play al iniciar y lo cierra al salir.
  - ACL: solo el usuario actual. Sin red y sin puertos abiertos.
- Mensajes JSON, uno por línea (`\n`), en UTF-8.
- Atic intenta conectarse **solo cuando la sesión SMTC actual es la de
  Atic-play** (AUMID `Ciat.AticPlay`). Si no conecta, reintenta con espera
  creciente y se queda con SMTC.

### Protocolo (v1)

Saludo, lo primero que manda Atic-play:

```json
{"type":"hello","app":"atic-play","version":1,"features":["like","queue","lyrics","open"]}
```

Estado, que Atic-play envía al conectarse y en cada cambio (no por tick: la
posición la sigue sacando de SMTC):

```json
{"type":"state","track":{"id":"dQw4w9WgXcQ","title":"…","artists":"…","album":"…",
 "duration":213,"liked":true},"playing":true,"shuffle":false,"repeat":"off",
 "status":"playing","upcoming":[{"id":"…","title":"…","artists":"…"}]}
```

- `status` puede ser `playing`, `paused`, `loading`, `offline` o `no_output`.
- `upcoming` lleva como máximo 10 elementos.

Letra, una vez por canción cuando Atic-play la tiene:

```json
{"type":"lyrics","track":"dQw4w9WgXcQ","synced":true,"lines":[{"at":12.4,"text":"…"}]}
```

Órdenes de Atic (Atic-play responde `{"type":"ack","id":N}` o
`{"type":"error","id":N,"message":"…"}`):

```json
{"type":"like","id":1,"liked":true}
{"type":"play_upcoming","id":2,"index":3}
{"type":"set_shuffle","id":3,"on":true}
{"type":"cycle_repeat","id":4}
{"type":"open","id":5,"what":"now_playing"}
```

En `open`, `what` puede ser `now_playing`, `album`, `artist` o `queue`.

**Reglas:**

- Campos desconocidos se ignoran.
- Una versión mayor distinta: el cliente se desconecta y vuelve a SMTC.
- Nada de datos personales de la cuenta (nombre, foto) por el canal.
- Las escrituras en la cuenta de YouTube (me gusta, playlists) las hace solo
  Atic-play, con su sesión.

### Dónde va el código

| Lado | Archivo nuevo | Engancha en |
|---|---|---|
| Atic-play | `crates/desktop/src/atic_bridge.rs` (servidor del pipe, hilo propio como `media_session.rs`) | Observa `Playback` y `Account`; las órdenes llegan como eventos al hilo de UI, igual que `MediaEvent` |
| Atic | `prototypes/pill-gpui/src/media/atic_play.rs` (cliente) | `media.rs`: si la fuente es Atic-play y el canal está vivo, suma corazón, cola y abrir; la letra recibida reemplaza la de LRCLIB |

## 5. Fase 4 (ideas, sin compromiso)

- **Celular:** Atic ya tiene canal con la app del teléfono (`atic-sync`). Las
  órdenes de la fase 3 pueden llegar desde el celular a través de Atic, sin que
  Atic-play hable con el teléfono.
- **Atajos globales:** si Atic ya tiene atajos globales, que "me gusta" global
  pase por el canal.
- **Instalador:** detectar la otra app y ofrecer activar la integración (solo un
  aviso; no hay nada que configurar).

## 6. Checklist de pruebas

**Con solo Atic-play instalado:**

- [ ] Reproduce, teclas multimedia y "Ahora suena" de Windows funcionan.
- [ ] No hay errores en el log por el pipe sin cliente.

**Con solo Atic instalado:**

- [ ] Ahora suena funciona con Spotify o el navegador como siempre.

**Con las dos instaladas:**

- [ ] Ahora suena muestra "Atic-play" con su ícono, título, carátula y progreso.
- [ ] Play/pausa, siguiente, anterior y arrastrar el progreso desde la pill
      funcionan.
- [ ] Cambiar la salida desde la pill **no pausa** Atic-play (arreglo 2.1).
- [ ] Desconectar los audífonos **sí pausa**.
- [ ] Volumen por app desde la pill cambia solo Atic-play.
- [ ] (Fase 3) El corazón refleja y cambia el me gusta; la cola coincide con la
      de Atic-play; "abrir" trae la ventana en la página correcta.
- [ ] (Fase 3) Cerrar Atic-play con la pill conectada: la pill vuelve a SMTC
      sin errores. Reabrirlo: se reconecta sola.

## Orden sugerido

1. **2.1** Distinguir desconexión de cambio de salida. Solo toca Atic-play y es
   lo más visible.
2. **2.2** AUMID y nombre, en los dos repos (cambios pequeños).
3. Decidir con Christian qué funciones de la sección 3 valen la pena.
4. Solo entonces, la **fase 3** (canal propio).
