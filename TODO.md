# filetag – backlog

## Ideas / future features

### Range tags
Tags die een ordinale of continue waarde bijhouden van een zelf te definiëren schaal.
Voorbeelden:
- Waardering 1–10 voor een tekening, waarbij de gebruiker ook named ranges kan definiëren (bijv. 1–3 = onvoldoende, 4–6 = voldoende, 7–10 = goed)
- Lichtintensiteit als schuifje
- Kleurtint (hue-waarde)

Eisen:
- Fuzzy/range queries in de query-taal: `rating>=7`, `rating:4-6`, `hue:warm`
- Named ranges zelf te definiëren en opslaan
- UI: slider of numeriek invoerveld naast de tag-chip
- Filterable via `filetag find rating>=7`

### Taggen binnen archieven
Archieven (zip, 7z, rar, cbz, cbr, tar, …) kunnen in twee modi getagt worden:

1. **Archief als geheel** – het archief-bestand zelf krijgt tags, zoals nu
2. **Bestanden binnen het archief** – individuele entries in het archief krijgen hun eigen tags in de database (virtuele paden, bijv. `archive.cbz!cover.jpg`)

Overwegingen:
- Virtueel pad als sleutel in de `files`-tabel (`archief.cbz::entry.jpg` of vergelijkbaar)
- Preview van entries in de web-UI (afbeeldingen, tekst)
- `file_id` voor entries binnen een archief: hash van inhoud (geen inode beschikbaar)
- CLI: `filetag tag archive.zip::subfile.txt -t tagname`

### Meer preview-formaten (web-UI)
Meeste afbeeldingen werken al via `<img>`. Ontbrekend:

- **Video** – meer containers/codecs (mkv, avi, mov, wmv) via browser-native `<video>`; melding als codec niet ondersteund wordt
- **Audio** – waveform-weergave of simpelweg `<audio>` voor mp3/flac/ogg/opus/aac
- **ZIP / archief** – inhoudsopgave tonen (bestandsnamen, groottes); eventueel koppelen aan in-archief-taggen (zie hierboven)
- **PDF** – eerste pagina renderen via `<canvas>` + PDF.js (optioneel, als dependency)
- **Tekst/code** – plain-text bestanden tonen met syntax-highlighting (bijv. via highlight.js)
- **SVG** – inline renderen (werkt al deels via `<img>`, maar interactieve SVG kan meer)
- Fallback: bestandsgrootte + extensie + "geen preview beschikbaar"
