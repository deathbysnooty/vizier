# Draw bank: the doodle pack for "Guess the Word"

The bot renders a human doodle in a Discord channel and members type what they
think it is; the first correct guess wins 2 points. `!hint` shows a **second
doodle of the same word** plus the word's first letter, `!skip` moves on.

This directory holds the doodles and the accepted answers. Nothing here is
generated at runtime: the bot reads two files at startup and is ready.

## Source and attribution

Every doodle is a real drawing by a real person, taken from Google's
**Quick, Draw!** dataset (the `simplified` per-category NDJSON files at
`https://storage.googleapis.com/quickdraw_dataset/full/simplified/<category>.ndjson`).
The dataset is published by Google under **Creative Commons Attribution 4.0
International (CC BY 4.0)**, which requires that we credit it wherever the
doodles appear.

**Put this exact line in the game's help text (and in `!guess` / rules output):**

```
Doodles from Google's Quick, Draw! dataset (https://quickdraw.withgoogle.com/data), used under CC BY 4.0.
```

Only drawings the original game's own classifier accepted (`"recognized": true`)
were sampled, so every doodle here was good enough that a machine could name it.

## Files

| file | what it is |
| --- | --- |
| `words.json` | the word list: display word, accepted answers, and where that word's doodles live in the pack |
| `doodles.bin` | all 9,600 doodles, one byte per coordinate |

### `words.json`

```json
{
 "format": "QDPACK1",
 "pack": "doodles.bin",
 "source": "Google Quick, Draw! dataset (simplified), CC BY 4.0",
 "attribution": "Doodles from Google's Quick, Draw! dataset (CC BY 4.0).",
 "words": 240,
 "doodles": 9600,
 "index": [
  {"word": "hat", "answers": ["hat", "cap", "topi", "sun hat"],
   "count": 40, "offset": 366810, "bytes": 2268},
  ...
 ]
}
```

- `word` — what the bot announces when the round ends, and the word `!hint`
  takes its first letter from. Always also present as `answers[0]`.
- `answers` — every spelling a reasonable member might type, **canonical one
  first**. Match a guess by normalising both sides (see below).
- `count` — how many doodles this word has (40 for every word).
- `offset` / `bytes` — this word's slice of `doodles.bin`, as absolute byte
  positions in the file. The slice contains exactly `count` doodles back to back.

### `doodles.bin` — format `QDPACK1`

```
byte 0..8      magic, the 8 bytes  51 44 50 41 43 4b 01 0a   ("QDPACK\x01\n")
byte 8..EOF    the word blocks, in the same order as words.json's "index"

each word block is `count` doodles, back to back; each doodle is:

  u8                stroke_count           (1..=25)
  repeated stroke_count times:
    u8              point_count            (1..=255)
    point_count x u8   the x coordinates   (0..=255)
    point_count x u8   the y coordinates   (0..=255)
```

No length prefixes, no padding, no endianness to worry about: everything is a
single byte. x grows right, y grows **down**. A drawing's points already fill a
0-255 box with the longer side at 255, so the renderer only has to scale the
box to the canvas.

#### Worked example

The second doodle of `hat` starts at byte 366872 and is 27 bytes long:

```
02  04 00 1f 89 ff  71 6b 61 50  08 52 47 3f 50 ab b1 b3 ab  6c 4a 11 0b 00 01 0a 65
```

Read it:

| bytes | meaning |
| --- | --- |
| `02` | 2 strokes |
| `04` | stroke 1 has 4 points |
| `00 1f 89 ff` | its x values: 0, 31, 137, 255 |
| `71 6b 61 50` | its y values: 113, 107, 97, 80 |
| `08` | stroke 2 has 8 points |
| `52 47 3f 50 ab b1 b3 ab` | its x values: 82, 71, 63, 80, 171, 177, 179, 171 |
| `6c 4a 11 0b 00 01 0a 65` | its y values: 108, 74, 17, 11, 0, 1, 10, 101 |

So: `[[[0,31,137,255],[113,107,97,80]], [[82,71,63,80,171,177,179,171],[108,74,17,11,0,1,10,101]]]`
— one long stroke for the brim, one arc for the crown.

### Reading it in Rust

`words.json` is ~42 KB of serde, and `doodles.bin` is under a megabyte, so read
both whole at startup and slice. There is no parsing to do beyond walking bytes:

```rust
// blob: Vec<u8> of doodles.bin, entry: one "index" element from words.json
let mut p = entry.offset;
let mut doodles = Vec::with_capacity(entry.count);
for _ in 0..entry.count {
    let n_strokes = blob[p] as usize; p += 1;
    let mut strokes = Vec::with_capacity(n_strokes);
    for _ in 0..n_strokes {
        let n = blob[p] as usize; p += 1;
        let xs = &blob[p..p + n];
        let ys = &blob[p + n..p + 2 * n];
        p += 2 * n;
        strokes.push((xs, ys));       // borrow straight out of the blob
    }
    doodles.push(strokes);
}
debug_assert_eq!(p, entry.offset + entry.bytes);
```

Render each stroke as one rounded polyline (round caps **and** round joins) in
near-black on a light ground; that is what the QC renderer did and what the
contact sheets were judged on. Stroke width around `canvas / 90` looks right at
600x600. A stroke with a single point is a legitimate dot (an eye, a freckle) —
draw it as a filled circle, don't skip it.

### Matching a guess

Normalise **both** the guess and each entry in `answers` the same way:
lowercase, then drop every character that is not `a-z` or `0-9`. That makes
`Ice-Cream`, `ice cream` and `icecream` the same string, so the answer lists
don't need to carry spacing variants. Compare for equality — no fuzzy matching,
no substring matching (a substring match would let "car" win a round whose word
is "police car").

```rust
fn normalise(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}
```

The same normalised answer can belong to more than one word on purpose — "boat"
is accepted for `sailboat`, `speedboat` and `cruise ship`. Rounds are
independent, so this costs nothing and rescues a lot of right answers.

## What is in the pack

- **240 words**, curated down from the dataset's 345 categories.
- **40 doodles per word**, 9,600 in total.
- **965 KB** on disk (922 KB `doodles.bin` + 43 KB `words.json`).

## Regenerating

The build scripts are throwaway and live in the scratchpad, not in the repo;
they are short enough to restate. The pipeline is four steps:

1. **Curate.** A `KEEP` map of `dataset category -> (display word, [answers])`
   and a `DROP` map of `category -> reason`, together covering all 345
   categories (`curl https://raw.githubusercontent.com/googlecreativelab/quickdraw-dataset/master/categories.txt`).
2. **Sample.** For each kept category, `HEAD` the NDJSON for its size, then pull
   three 900 KB byte ranges (at 0%, 40%, 75%) with `curl -r`, so the sample
   spans different drawers and countries. **Never download a whole file** —
   several are hundreds of megabytes. Parse line by line, keep only
   `recognized: true`, and hold nothing but the chosen stroke arrays in memory.
   Discard a drawing if any of these is true:
   - more than 25 strokes, or fewer than 12 / more than 450 points total
     (scribbles and blanks);
   - its bounding box does not reach 200 on its longer side, or is under 12 on
     its shorter side (degenerate lines);
   - 10+ strokes averaging under 4 points each (handwriting, signatures);
   - it *looks like writing*: a band wider than it is tall, made of 4+ short
     strokes that sit side by side with little horizontal overlap. People do
     sometimes just write the word — this catches that.

   From the survivors, sort by detail (stroke count, then point count), **drop
   the sparsest 20%** — that is where the plain circles, bare rectangles and
   single arcs live, and nobody can guess those — then take 40 evenly strided
   across the rest so a word gets simple and detailed drawings in proportion.
3. **Quality-check by rendering.** Draw contact sheets and *look at them*: one
   doodle from every word first, then a full row per suspect word. Drop any word
   whose doodles are not nameable, and any word that collides with another kept
   word. This step removed 16 words; see the dropped table below.
4. **Pack.** Concatenate as described above, recording `offset`/`bytes` per
   word, and verify by decoding the pack back and comparing it to the sampled
   JSON byte for byte.

Bumping the doodles-per-word is cheap — at 40 the pack is under 1 MB, so 100
per word would still be about 2.4 MB.

## Curation

### Why these 240

Kept: things any member of a Desi Discord server would name in English, in one
obvious word, that survive being drawn in fifteen seconds by a stranger.

The accepted-answer lists lean hard on Indian English, because the everyday word
here often is not the American one: **torch** not flashlight, **slippers** /
**chappal** not flip flops, **mobile** not cell phone, **bagpack** as people
actually spell it, **signal** for traffic light, **injection** for syringe,
**biscuit** for cookie, **half pant** for shorts, **rubber** for eraser,
**photo frame**, **JCB**, **sand clock**, **maachis**, **pyaz**, **aloo**.
Common misspellings are in the lists too (`recieve`-class errors like
`helicoptor`, `sandwhich`, `dumbell`, `pengiun`) — a right answer rejected is
the worst thing that can happen in this game, so the lists are deliberately
generous. Adding more is safe: edit `answers` in `words.json`, no rebuild needed.

### Dropped categories

105 of the 345 categories are not in the pack. Grouped:

- **Ambiguous as a drawing** (several equally right names): `bat` (animal vs
  cricket bat), `pool` (swimming pool vs pool table), `mouse` was kept only
  because both readings are accepted, `map`, `passport`, `postcard`.
- **Collides with a word that was kept**: `alarm clock` vs `clock`,
  `school bus` vs `bus`, `teddy-bear` vs `bear`, `coffee cup`/`cup` vs `mug`,
  `oven` vs `microwave`, `van`/`pickup truck` vs `truck`, `smiley face` vs
  `face`, `birthday cake` vs `cake`, `fan` vs `ceiling fan`.
- **US-specific or unfamiliar in India**: `mailbox`, `raccoon`, `fire hydrant`,
  `fireplace`, `power outlet`, `barn`, `rake`, `popsicle`, `cooler` (means an
  air cooler here), `dresser`, `stereo`, `hot tub`, `canoe`, `baseball`,
  `baseball bat`, `hockey puck`, `golf club`, `steak`, `string bean`,
  `asparagus`, `hedgehog`, `blackberry`, `blueberry`, `harp`, `cello`,
  `clarinet`, `trombone`, `sleeping bag`, `snorkel`, `diving board`.
- **Multi-word names nobody types exactly**: `The Great Wall of China`,
  `The Mona Lisa`, `aircraft carrier`, `roller coaster`, `waterslide`,
  `garden hose`, `paint can`.
- **Abstract or unrecognisable**: `line`, `squiggle`, `zigzag`, `camouflage`,
  `animal migration`, `grass`, `bush`, `ocean`, `river`, `pond`, `beach`,
  `garden`, `spreadsheet`, `stitches`, `hurricane`, `yoga`.
- **Body parts that are a shapeless blob on their own**: `arm`, `leg`, `elbow`,
  `knee`, `foot`, `toe`, `finger`, `beard`, `goatee`.
- **Failed the render check** — these read fine on paper but their actual
  doodles do not work; 16 words were cut at this step, marked `render QC:` in
  the table.
- `stop sign`: the drawing literally spells the answer.

Full list with reasons:

| dropped category | why |
| --- | --- |
| `The Great Wall of China` | long multi-word name nobody types exactly (explicitly excluded) |
| `The Mona Lisa` | amateur doodles of it are unrecognisable |
| `aircraft carrier` | multi-word, unfamiliar; the doodles are indistinguishable from 'ship' |
| `alarm clock` | collides with 'clock' - a player typing 'clock' would be wrongly rejected |
| `animal migration` | abstract scene, no single obvious name |
| `anvil` | cartoon-only object, most Indian members would not name it |
| `arm` | body part indistinguishable from 'leg'/'hand' when drawn |
| `asparagus` | unfamiliar vegetable in India and a shapeless drawing |
| `barn` | US/Western farm building; would be typed as 'house' or 'shed' |
| `baseball` | US sport; drawn it is just a ball, people would type 'ball' or 'cricket ball' |
| `baseball bat` | US sport; collides with 'bat' the animal and with cricket bat |
| `bat` | fatally ambiguous - animal vs cricket bat, both equally right in India |
| `beach` | scene, several equally right names (sea, sand, beach, island) |
| `beard` | drawn as a face with hair; collides with 'face', 'moustache', 'goatee' |
| `birthday cake` | collides with 'cake'; multi-word |
| `blackberry` | unfamiliar fruit; drawn it is an unrecognisable berry blob |
| `blueberry` | unfamiliar fruit; drawn it is an unrecognisable berry blob |
| `boomerang` | shape is indistinguishable from a banana or a crescent |
| `bottlecap` | obscure object, doodles are unrecognisable circles |
| `bracelet` | render QC: the doodles are plain ovals - indistinguishable from a ring, a bangle outline or a circle |
| `broccoli` | drawn it looks like a tree or a bush |
| `bush` | abstract scribble, indistinguishable from 'tree' or 'grass' |
| `camouflage` | abstract pattern, no nameable object |
| `cannon` | render QC: the doodles are a barrel on a plank; nobody guesses 'cannon' from that |
| `canoe` | unfamiliar in India; would be typed as 'boat', colliding with sailboat/speedboat |
| `cello` | unfamiliar in India and indistinguishable from violin/guitar doodles |
| `chandelier` | render QC: the doodles are abstract hanging lines, not a nameable object |
| `clarinet` | unfamiliar in India and indistinguishable from a flute or recorder doodle |
| `coffee cup` | collides with 'mug' and 'cup' |
| `compass` | render QC: most doodles are a plain circle with a scratch inside - reads as 'circle' or 'clock' |
| `cooler` | 'cooler' means an air cooler in India, not an ice box - wrong mental image |
| `crayon` | collides with 'pencil' and 'marker'; the doodles are identical sticks |
| `cup` | collides with 'mug'; the doodles are a mix of cups, glasses and plastic tumblers |
| `dishwasher` | uncommon appliance in India; drawn it is a box like an oven or washing machine |
| `diving board` | multi-word, obscure, abstract drawing |
| `dresser` | US furniture word; Indians say cupboard / chest of drawers |
| `elbow` | body part, unrecognisable as a doodle |
| `eraser` | render QC: every doodle is a plain parallelogram |
| `fan` | handheld fan collides with 'ceiling fan', which is the Indian meaning of 'fan' |
| `fence` | render QC: the doodles are a row of vertical lines - could equally be a gate, a jail or a grid |
| `finger` | collides with 'hand' |
| `fire hydrant` | US street furniture, not part of Indian daily life |
| `fireplace` | Western house feature, rarely seen in India |
| `foot` | body part, collides with 'leg' and 'shoe' |
| `garden` | scene, no single right answer |
| `garden hose` | multi-word and drawn as an ambiguous squiggle |
| `goatee` | obscure word, collides with 'beard' and 'moustache' |
| `golf club` | US-leaning sport, and the doodle is indistinguishable from a hockey stick |
| `grass` | abstract scribble |
| `harp` | unfamiliar instrument in India, doodles are ambiguous |
| `hedgehog` | unfamiliar in India, commonly confused with porcupine |
| `hockey puck` | obscure; drawn it is a plain oval |
| `hockey stick` | the doodle is a plain bent stick, indistinguishable from a golf club or a walking stick |
| `hot tub` | US-specific; collides with 'bathtub' |
| `hurricane` | abstract spiral, collides with 'tornado' |
| `jail` | render QC: the doodles are vertical bars, identical to the 'fence' doodles |
| `knee` | body part, unrecognisable as a doodle |
| `leg` | body part indistinguishable from 'arm' |
| `line` | abstract, not a thing |
| `mailbox` | US-specific street furniture (explicitly excluded) |
| `map` | drawn as a blob with lines, no single right answer |
| `marker` | collides with pen / pencil / crayon; doodles are identical |
| `matches` | render QC: half the doodles are a bare stick and the rest read as a paintbrush, which is a kept word |
| `nail` | iron nail vs fingernail ambiguity, and a thin line is unguessable |
| `ocean` | abstract scene |
| `octagon` | visually indistinguishable from 'hexagon' in a hand doodle |
| `oven` | collides with 'microwave'; both are drawn as a box with a door |
| `paint can` | multi-word and indistinguishable from 'bucket' |
| `paper clip` | render QC: the doodles are plain nested loops, indistinguishable from a ring or a bracelet |
| `passport` | drawn as a plain rectangle with squiggles, unguessable |
| `peas` | render QC: scattered blobs; only a minority show a recognisable pod |
| `pickup truck` | US-leaning; collides with 'truck' and 'van' |
| `pillow` | render QC: every doodle is a plain rounded rectangle |
| `pliers` | render QC: the doodles look exactly like scissors, which is a kept word |
| `pond` | abstract scene |
| `pool` | swimming pool vs pool table - two equally right answers |
| `popsicle` | US word; Indians say ice candy / ice lolly / kulfi, none of which is 'popsicle' |
| `postcard` | drawn as an ambiguous rectangle |
| `potato` | render QC: every doodle is a featureless blob |
| `power outlet` | US socket shape, does not match Indian plug points |
| `raccoon` | US animal, unfamiliar in India (explicitly excluded) |
| `rake` | Western garden tool, unfamiliar in India |
| `remote control` | render QC: a rectangle with a few dots - identical to a phone or a calculator doodle |
| `river` | abstract scene |
| `roller coaster` | multi-word and drawn as an abstract scribble |
| `school bus` | collides with 'bus' |
| `screwdriver` | render QC: the doodles are a handle with a shaft, indistinguishable from the kept word 'knife' |
| `sleeping bag` | multi-word, obscure in India, drawn as a blob |
| `smiley face` | collides with 'face', which is kept with 'smiley' as an accepted answer |
| `snorkel` | obscure object, unrecognisable doodle |
| `spreadsheet` | abstract grid, collides with 'calendar' and 'table' |
| `squiggle` | abstract, not a thing |
| `steak` | Western food; drawn it is an unrecognisable blob |
| `stereo` | US word; collides with 'radio' and 'speaker' |
| `stitches` | abstract, unpleasant, and unrecognisable |
| `stop sign` | the drawing literally spells the answer, so it is not a game |
| `string bean` | US name; Indians say beans, and the doodle is a plain curved line |
| `teddy-bear` | collides with 'bear'; hyphenated name nobody types exactly |
| `tiger` | render QC: a striped animal that most people would type as 'cat'; the sample also contained someone writing the word |
| `toe` | body part, unrecognisable as a doodle |
| `trombone` | unfamiliar in India and indistinguishable from 'trumpet' |
| `van` | collides with 'truck', 'bus' and 'car' |
| `waterslide` | multi-word and drawn as an abstract scribble |
| `yoga` | stick-figure poses; almost nobody would type the word 'yoga' |
| `zigzag` | abstract, not a thing |

### Words a human should rule on

These are in the pack but are the shakiest. Say the word and I will cut them:

- **snowflake** — the doodles are six-spoke asterisks. Fair, but someone may
  well type "star" (a separate word, drawn as a five-point outline).
- **frog** — a good minority of the doodles are round faces that read as "bear",
  which is also a kept word.
- **microwave** — a box with an inner window; "oven" is accepted, but someone
  might reasonably type "tv".
- **basketball** / **football** — both are a ball with lines on it. The seam
  patterns do differ, and both accept "ball"-ish alternatives, but they are the
  closest pair in the pack.
- **swan** — accepts "duck", and `duck` is a separate word. Deliberate, but it
  makes `swan` easy.
- **underwear** — accepted answers include `chaddi`. Fine for an adult server,
  but it is your call.
- **gun** (dataset category `rifle`) — a weapon doodle. Harmless, but flagging it
  since the server is a mixed adult community.
- **palm tree** — plain `tree` is deliberately *not* accepted, so that `tree` and
  `palm tree` stay distinct. Add it if you would rather be generous.
- **flamingo**, **lobster** — the only words whose drawings need a fairly
  specific animal to come to mind; `lobster` accepts prawn/shrimp/jhinga.

## Accepted answers, all 240 words

The canonical word is bold; everything after it is also accepted. Matching
ignores case, spaces and punctuation, so `t shirt` also covers `T-Shirt` and
`tshirt`.

| word (`!hint` reveals its first letter) | also accepted |
| --- | --- |
| **eiffel tower** | eiffel, the eiffel tower, effiel tower, eifel tower, paris tower, tower |
| **aeroplane** | airplane, plane, aircraft, jet, flight |
| **ambulance** | ambulance van |
| **angel** | fairy, pari, angle |
| **ant** | chiti, cheenti, chinti |
| **apple** | seb, aapple |
| **axe** | ax, hatchet, kulhadi, kulhari |
| **backpack** | bagpack, bag, school bag, rucksack, knapsack |
| **banana** | kela, keley, bananas |
| **bandage** | band aid, plaster, patti, dressing, bandege |
| **basket** | tokri, tokari, basket tokri, wicker basket |
| **basketball** | basket ball hoop, hoop |
| **bathtub** | tub, bath, bathing tub |
| **bear** | bhalu, grizzly, grizzly bear, polar bear |
| **bed** | palang, palang bed, cot, bedstead |
| **bee** | honeybee, bumblebee, madhumakhi |
| **belt** | waist belt, leather belt |
| **bench** | park bench, sitting bench |
| **bicycle** | cycle, bike, push bike, bycycle |
| **binoculars** | binocular, binocs, doorbeen, dooerbeen, field glasses |
| **bird** | chidiya, chiriya, sparrow, birdie |
| **book** | notebook, kitab, kitaab, diary, novel, copy |
| **bowtie** | bow, ribbon |
| **brain** | dimaag, dimag, mind |
| **bread** | loaf, bread loaf, double roti, pav, slice of bread, toast |
| **bridge** | pul, pull bridge, flyover, brige |
| **broom** | broomstick, jhadu, jhaadu, jharu |
| **bucket** | balti, baalti, pail |
| **bulldozer** | jcb, digger, excavator, earth mover, dozer |
| **bus** | school bus, public bus |
| **butterfly** | titli |
| **cactus** | cacti, nagfani, naagfani, cactas |
| **cake** | birthday cake, pastry, cup cake |
| **calculator** | calc, calculater |
| **calendar** | calender, date sheet, wall calendar |
| **camel** | oont, unt, ount |
| **camera** | dslr, kamera, photo camera |
| **campfire** | fire, bonfire, aag |
| **candle** | mombatti, mombati, candles |
| **car** | gaadi, gadi, automobile, sedan, motor car |
| **carrot** | gajar, carrots |
| **castle** | fort, qila, kila, palace, mahal, castel |
| **cat** | billi, kitten, kitty, billy cat |
| **ceiling fan** | fan, pankha, panka, celing fan, roof fan |
| **mobile phone** | mobile, phone, cellphone, smartphone, iphone, handset |
| **chair** | kursi, kurshi, stool |
| **church** | cathedral, chapel, girja, girja ghar |
| **circle** | round, gola, circel, ring |
| **clock** | watch, ghadi, ghari, wall clock, time, alarm clock |
| **cloud** | clouds, badal, baadal |
| **computer** | pc, desktop, monitor, screen, cpu, computor |
| **cookie** | biscuit, biscuits, cookies, parle g |
| **couch** | sofa, settee, sofa set, divan |
| **cow** | gaay, gai, gau, cattle, calf |
| **crab** | kekda, kekra |
| **crocodile** | croc, alligator, magarmach, magarmachh, gharial |
| **crown** | taj, tiara, king crown |
| **cruise ship** | cruise, ship, cruise liner, boat |
| **diamond** | gem, jewel, heera, hira, diamond stone |
| **dog** | kutta, puppy, pup, doggy |
| **dolphin** | dolfin |
| **donut** | doughnut, doughnuts, donuts |
| **door** | darwaza, darwaaza, gate, doorway |
| **dragon** | dragan, dinosaur |
| **drill** | drill machine, drilling machine, power drill, driller |
| **drums** | drum, drum set, drum kit, dholak, dhol |
| **duck** | batak, batakh, ducky, duckling |
| **dumbbell** | dumbell, dumble, weights, weight, barbell, gym weight |
| **ear** | kaan, ears |
| **elephant** | hathi, haathi, elefant |
| **envelope** | letter, mail, lifafa, lifaafa, envelop |
| **eye** | eyes, aankh, ankh |
| **glasses** | spectacles, specs, eyeglasses, sunglasses, chashma, chasma, goggles |
| **face** | smiley, smiley face, smile, emoji, chehra, happy face |
| **feather** | pankh, quill, feathers |
| **fire truck** | fire engine, fire brigade, fire brigade truck |
| **fish** | machli, machhli, machhi |
| **flamingo** | flamingoes, flemingo |
| **torch** | flashlight, torch light, hand torch, battery torch |
| **slippers** | slipper, chappal, chappals, chapal, flip flops, flip flop, sandals, sandal, hawai chappal |
| **floor lamp** | lamp, lampshade, table lamp, standing lamp, light |
| **flower** | phool, phul, rose, daisy, flowers |
| **flying saucer** | ufo, alien ship, spaceship, saucer, alien spaceship |
| **fork** | kanta, kaanta, table fork |
| **frog** | toad, mendak, mendhak |
| **frying pan** | pan, tawa, tava, kadhai, kadai, karahi, skillet, fry pan |
| **giraffe** | giraf, jiraffe, zaraffa |
| **grapes** | grape, angoor, angur |
| **guitar** | gitar, bass guitar |
| **burger** | hamburger, cheeseburger, beef burger |
| **hammer** | hathoda, hathoda hammer, mallet, hamer |
| **hand** | palm, haath, hath, fingers, five fingers |
| **hat** | cap, topi, sun hat |
| **headphones** | headphone, headset, earphones |
| **helicopter** | chopper, helicoptor |
| **helmet** | helmate, hat helmet |
| **hexagon** | hexagone, six sided shape, hexagan |
| **horse** | ghoda, pony, stallion, mare |
| **hospital** | clinic, aspatal, hospitle, medical |
| **hot air balloon** | air balloon, balloon, parachute balloon |
| **hot dog** | sausage, sausage roll |
| **hourglass** | sand clock, sand timer, sandglass, sand watch |
| **house** | home, ghar, hut, makan, cottage |
| **house plant** | plant, pot plant, potted plant, flower pot, gamla, indoor plant |
| **ice cream** | cone, ice cream cone, softy, soft serve |
| **jacket** | coat, blazer, hoodie, overcoat, windcheater |
| **kangaroo** | kangroo |
| **key** | chaabi, chabi, chaabhi, keys |
| **keyboard** | computer keyboard, keybord |
| **knife** | chaku, chakku, dagger, blade, kitchen knife |
| **ladder** | seedhi, sidhi, seedi, step ladder |
| **lantern** | lalten, laltein, lamp, oil lamp, hanging lantern |
| **laptop** | macbook, notebook computer, computer |
| **leaf** | patta, pata, leaves, leafs |
| **bulb** | light bulb, light, lamp, idea, led bulb |
| **lighter** | cigarette lighter, gas lighter, liter |
| **lighthouse** | light tower |
| **lightning** | thunder, bolt, lightning bolt, bijli, thunderbolt, lighting |
| **lion** | sher, lioness |
| **lipstick** | lipgloss, lip balm |
| **lobster** | prawn, shrimp, crayfish, jhinga |
| **lollipop** | lolly pop, lolly, candy, chupa chups |
| **megaphone** | loudspeaker, speaker, bullhorn, horn |
| **mermaid** | jalpari, merman |
| **microphone** | mic, mike, mic stand |
| **microwave** | microwave oven, oven, otg |
| **monkey** | bandar, ape, chimp, chimpanzee, gorilla, langur |
| **moon** | chand, chaand, crescent, half moon, cresent |
| **mosquito** | machar, machhar, musquito, fly, insect |
| **motorbike** | motorcycle, bike, scooter, scooty, bullet, two wheeler |
| **mountain** | mountains, hill, hills, pahad, pahaad, mountan |
| **mouse** | rat, chuha, chooha, computer mouse, mice |
| **moustache** | mustache, mooch, moonch, moustach, mustach |
| **mouth** | lips, lip, smile, muh, honth |
| **mug** | cup, coffee mug, coffee cup, tea cup, chai cup, glass, beer mug |
| **mushroom** | mushrooms, khumb, toadstool, khumbi |
| **necklace** | chain, haar, har, locket, pendant, neck chain |
| **nose** | naak, nak |
| **octopus** | squid, octapus |
| **onion** | pyaz, pyaaz, piyaz, onions |
| **owl** | ullu, uluu |
| **paintbrush** | brush, painting brush, art brush |
| **palm tree** | coconut tree, palm, nariyal tree, coconut palm, date tree |
| **panda** | panda bear |
| **pants** | pant, trousers, trouser, jeans, slacks |
| **parachute** | paraglider, paragliding |
| **parrot** | tota, totta, macaw |
| **peanut** | peanuts, groundnut, moongfali, mungfali, mungphali |
| **pear** | nashpati, naspati |
| **pencil** | pensil, pen, lead pencil |
| **penguin** | pengiun, pengu |
| **piano** | keyboard, grand piano, pianoo |
| **photo frame** | picture frame, frame, painting, photo, wall frame |
| **pig** | suar, piggy, hog, swine, piglet |
| **pineapple** | ananas, anaanas, anannas |
| **pizza** | pizza slice, piza |
| **police car** | police, cop car, police van, police jeep |
| **purse** | handbag, bag, wallet, ladies bag, clutch |
| **rabbit** | bunny, khargosh, hare |
| **radio** | transistor, boombox, radio set, fm radio |
| **rain** | raining, barish, baarish, rainy, rainfall, rain cloud |
| **rainbow** | indradhanush, indradhanus |
| **rhino** | rhinoceros, gainda, gaida, rhinocerous |
| **gun** | rifle, bandook, banduk, shotgun, riffle |
| **roller skates** | skates, skating shoes, roller blades, skate shoes |
| **sailboat** | boat, yacht, ship, sailing boat |
| **sandwich** | sandwhich, sandwitch, toast sandwich |
| **saw** | handsaw, aari, ari, hacksaw |
| **saxophone** | sax, saxaphone, saxophon |
| **scissors** | scissor, kainchi, kenchi, cutter, sissors |
| **scorpion** | bichhu, bichu, scorpian |
| **turtle** | sea turtle, tortoise, kachua, kachhua |
| **see saw** | seesaw ride |
| **shark** | sharks |
| **sheep** | lamb, goat, bhed, bakri, ram |
| **shoe** | shoes, sneaker, sneakers, boot, boots, juta, joota, jutta |
| **shorts** | short, half pant, half pants, boxers, nicker |
| **shovel** | spade, phavda, phawda, fawda, digger, garden spade |
| **sink** | washbasin, basin, hand wash basin, kitchen sink |
| **skateboard** | skating board, skate |
| **skull** | skeleton, khopdi, skull and bones |
| **skyscraper** | building, tower, high rise, apartment |
| **snail** | ghongha, slug |
| **snake** | saanp, sanp, serpent, cobra, python |
| **snowflake** | snow, ice crystal |
| **snowman** | frosty |
| **football** | soccer ball, soccer, ball, footbal |
| **sock** | socks, stocking, mojey, moja |
| **speedboat** | boat, motorboat |
| **spider** | makdi, makadi, makri, tarantula |
| **spoon** | chammach, chamach, chamcha, table spoon |
| **square** | box, chokor, squre |
| **squirrel** | gilhari, gilheri, chipmunk, squirel |
| **stairs** | staircase, steps, seedhi, stair |
| **star** | sitara, taara, tara, stars |
| **stethoscope** | stethescope, stethoscop, doctor stethoscope |
| **stove** | gas stove, chulha, chulah, burner, gas burner, cooktop |
| **strawberry** | strawberries |
| **streetlight** | lamp post, street lamp, pole light |
| **submarine** | sub, subarine |
| **suitcase** | briefcase, luggage, trolley bag, attache, bag |
| **sun** | sooraj, suraj, sunshine |
| **swan** | goose, hans, duck, geese |
| **sweater** | jumper, pullover, sweat shirt, hoodie, woolen |
| **swing** | swing set, jhoola, jhula, jhoolaa, swings |
| **sword** | talwar, talvaar, katana, blade, sord |
| **syringe** | injection, needle, sui, suii, siringe, injection needle |
| **t shirt** | tee shirt, shirt, tee, half sleeve shirt |
| **table** | desk, mez, mej, meaz, study table |
| **teapot** | kettle, chai pot, ketli, kettel |
| **telephone** | landline, phone, receiver, old phone, telefone |
| **tv** | television, telly, idiot box |
| **tennis racket** | tennis racquet, racket, racquet, badminton racket, raquet, bat racket |
| **tent** | camp, tambu, tambo, camping tent, tant |
| **toaster** | toast, bread toaster, toster, pop up toaster |
| **toilet** | commode, wc, western toilet, loo, toilet seat, latrine |
| **tooth** | teeth, daant, dant, molar |
| **toothbrush** | brush, tuthbrush |
| **toothpaste** | paste, colgate, tooth past |
| **tornado** | twister, cyclone, whirlwind, hurricane, tornedo |
| **tractor** | farm tractor |
| **traffic light** | signal, traffic signal, stop light, red light, traffic lights |
| **train** | rail, railway, locomotive, engine, rail gadi, railgaadi |
| **tree** | ped, pedh, plant, trees |
| **triangle** | tringle, triangel |
| **truck** | lorry, tempo, goods truck, van |
| **trumpet** | horn, bugle, trumpit |
| **umbrella** | chatri, chhatri, chhata, chata, umbrela |
| **underwear** | undies, briefs, chaddi, chadi, panty, boxers, innerwear |
| **vase** | flower vase, pot, flower pot, gamla, vaas, urn |
| **violin** | fiddle, voilin, vayolin |
| **washing machine** | washing machin, washer, clothes washer |
| **watermelon** | tarbooj, tarbuj, tarbuz, melon |
| **whale** | blue whale, wale |
| **wheel** | tyre, tire, wheel tyre, pahiya, car wheel |
| **windmill** | wind turbine, windmil, turbine |
| **wine bottle** | bottle, wine, beer bottle, liquor bottle, daru bottle |
| **wine glass** | glass, goblet, champagne glass, cocktail glass |
| **wrist watch** | watch, hand watch, ghadi, ghari |
| **zebra** | zeebra |
