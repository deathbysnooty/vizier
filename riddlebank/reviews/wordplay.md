# wordplay.jsonl review

Every riddle was solved blind before its answers were read. All 125 had the intended answer as the first solve (or as an accepted alias). JSON checked after the edits: 125 lines, ids unique, in order and unchanged, riddles no longer than 300 characters, hints no longer than 100, answers lower case and 1 to 3 words. Mix: 56 easy / 49 medium / 20 hard.

**Kept 113 / rewritten 6 / relabelled 6 / dropped 0**

## Rewritten
- wordplay-101 (dreamt): added `daydreamt`. A player could defend it, even though "dreamt" is the famous answer.
- wordplay-095 (thesaurus): the hint "A book of synonyms" repeated what the riddle already says. Replaced it with a nudge towards dinosaur names.
- wordplay-096 (irrelephant): the hint "Not relevant", plus the accepted `irrelevant`, gave the answer away. It now points at the elephant.
- wordplay-115 (barberqueue): added `barber queue`, the way most people will type it. The pun is kept because barber and queue are both everyday words in Indian English.
- wordplay-118 (shakespeare): added `shakes spear`. The pun is kept because Shakespeare is school-level knowledge and nervous javelin thrower leads to "shakes spear".
- wordplay-125 (swims): the old clue "what people do in a pool" pointed to "swim", which has 4 letters. The upside-down trick was only a check, so the old "hard" label wasn't honest. It's now a fill-the-gap clue ("She ___ ten laps…") where only "swims" fits, and it's relabelled hard → medium. SWIMS does read the same upside down: W flips to M.

## Relabelled
- wordplay-044 (nacho cheese): easy → medium. It hinges on "nacho" sounding like "not your", which needs a moment.
- wordplay-092 (alarm clock): medium → easy.
- wordplay-105 (minute): hard → medium. "Sixty seconds" gives it away quickly.
- wordplay-108 (four): hard → medium. It's quick to check.
- wordplay-113 (reality): hard → medium. "Opposite of fantasy" leads straight there, and the tea sound is a bonus. Kept.
- wordplay-124 (polish): hard → medium. Most Indian adults know "boot polish".

## Flagged items kept without changes
- wordplay-107 (forty): checked that f-o-r-t-y is in alphabetical order and no other number is. `fourty` and `40` stay as aliases.
- wordplay-093 (pouch potato): "couch potato" is common in Indian English, and a kangaroo on the sofa leads to "pouch". Fair at medium.
- wordplay-117 (drizzly bear): grizzly bear and drizzle are both well known, and the hint names the bear. Fair at hard.
- wordplay-119 (leek): `leak` is already accepted, so players who don't know the vegetable can still solve it.

## Note for the matcher
- wordplay-084 (fsh): if typo tolerance lets a 3-letter answer be off by one, then "fish" would count as correct. Make sure the tolerance is off for very short answers.
