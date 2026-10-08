# Machine translation benchmark: Mozilla (Firefox Translations) vs Opus-MT

NTREX-128, the first 100 sentences of each language (news, ~25 words a sentence); each sentence translated on its own; chrF against the human translation (higher is better). Mozilla: the Firefox engine (WebAssembly, one thread) with its newest models; Opus-MT: CTranslate2 8-bit, one thread, greedy, on each pair's shortest route (a direct model when there is one). Two hops: through English.

## with English

| pair | Mozilla chrF | route | ms/sentence | Opus-MT chrF | route | ms/sentence |
|---|---|---|---|---|---|---|
| es→en | 64.3 | 1 hop | 61 (p90 103) | 64.0 | 1 hop | 259 (p90 425) |
| fr→en | 51.4 | 1 hop | 64 (p90 113) | 52.4 | 1 hop | 614 (p90 1028) |
| de→en | 62.0 | 1 hop | 61 (p90 111) | 60.4 | 1 hop | 241 (p90 394) |
| it→en | 65.4 | 1 hop | 62 (p90 103) | 66.6 | 1 hop | 633 (p90 1048) |
| pt→en | 64.8 | 1 hop | 63 (p90 111) | 63.4 | 1 hop | 267 (p90 459) |
| nl→en | 58.7 | 1 hop | 61 (p90 104) | 55.9 | 1 hop | 413 (p90 525) |
| pl→en | 52.5 | 1 hop | 64 (p90 110) | 43.3 | 1 hop | 288 (p90 457) |
| uk→en | 54.9 | 1 hop | 32 (p90 60) | 47.7 | 1 hop | 269 (p90 431) |
| ru→en | 48.2 | 1 hop | 67 (p90 119) | 47.9 | 1 hop | 268 (p90 445) |
| zh→en | 55.7 | 1 hop | 55 (p90 93) | 51.2 | 1 hop | 267 (p90 441) |
| yue→en | 54.2 | 1 hop | 58 (p90 95) | - | - | - |
| ja→en | 48.7 | 1 hop | 50 (p90 79) | 40.3 | 1 hop | 232 (p90 372) |
| ko→en | 56.2 | 1 hop | 51 (p90 88) | 4.8 | 1 hop | 1421 (p90 4314) |
| cs→en | 62.8 | 1 hop | 61 (p90 106) | 58.7 | 1 hop | 253 (p90 426) |
| sk→en | 56.9 | 1 hop | 33 (p90 58) | 54.4 | 1 hop | 266 (p90 425) |
| ro→en | 59.2 | 1 hop | 35 (p90 61) | 56.1 | 1 hop | 332 (p90 480) |
| hr→en | 59.8 | 1 hop | 33 (p90 58) | 62.0 | 1 hop | 654 (p90 1101) |
| bg→en | 58.3 | 1 hop | 65 (p90 110) | 60.6 | 1 hop | 672 (p90 1054) |
| fi→en | 56.6 | 1 hop | 59 (p90 106) | 59.5 | 1 hop | 614 (p90 981) |
| sv→en | 61.8 | 1 hop | 28 (p90 49) | 48.6 | 1 hop | 171 (p90 297) |
| hu→en | 52.1 | 1 hop | 63 (p90 108) | 51.1 | 1 hop | 619 (p90 978) |
| da→en | 67.6 | 1 hop | 30 (p90 53) | 64.3 | 1 hop | 254 (p90 426) |
| et→en | 57.2 | 1 hop | 59 (p90 101) | 59.3 | 1 hop | 621 (p90 996) |
| lv→en | 57.1 | 1 hop | 35 (p90 62) | 56.8 | 1 hop | 649 (p90 1043) |
| lt→en | 59.5 | 1 hop | 32 (p90 57) | 62.0 | 1 hop | 631 (p90 1058) |
| sl→en | 58.9 | 1 hop | 62 (p90 111) | 60.5 | 1 hop | 624 (p90 1028) |
| el→en | 59.3 | 1 hop | 33 (p90 58) | 61.9 | 1 hop | 642 (p90 1038) |
| mt→en | 63.7 | 1 hop | 37 (p90 68) | 60.2 | 1 hop | 330 (p90 515) |
| en→es | 63.5 | 1 hop | 60 (p90 109) | 63.7 | 1 hop | 667 (p90 1145) |
| en→fr | 49.8 | 1 hop | 63 (p90 111) | 51.6 | 1 hop | 714 (p90 1169) |
| en→de | 57.2 | 1 hop | 62 (p90 108) | 54.5 | 1 hop | 250 (p90 423) |
| en→it | 61.0 | 1 hop | 59 (p90 104) | 60.4 | 1 hop | 652 (p90 1100) |
| en→pt | 58.3 | 1 hop | 59 (p90 105) | 58.6 | 1 hop | 643 (p90 1112) |
| en→nl | 55.4 | 1 hop | 59 (p90 103) | 53.6 | 1 hop | 285 (p90 521) |
| en→pl | 45.4 | 1 hop | 61 (p90 107) | 36.7 | 1 hop | 328 (p90 517) |
| en→uk | 57.4 | 1 hop | 62 (p90 112) | 40.4 | 1 hop | 261 (p90 457) |
| en→ru | 42.5 | 1 hop | 63 (p90 112) | 39.8 | 1 hop | 292 (p90 526) |
| en→zh | 24.8 | 1 hop | 51 (p90 90) | 19.3 | 1 hop | 225 (p90 320) |
| en→ja | 32.1 | 1 hop | 55 (p90 102) | 5.5 | 1 hop | 377 (p90 624) |
| en→ko | 28.9 | 1 hop | 53 (p90 95) | 1.9 | 1 hop | 1271 (p90 4328) |
| en→cs | 55.4 | 1 hop | 60 (p90 105) | 51.0 | 1 hop | 266 (p90 470) |
| en→sk | 51.1 | 1 hop | 61 (p90 105) | 46.8 | 1 hop | 265 (p90 462) |
| en→ro | 54.1 | 1 hop | 32 (p90 57) | 53.5 | 1 hop | 669 (p90 1201) |
| en→hr | 55.4 | 1 hop | 31 (p90 55) | 51.5 | 1 hop | 278 (p90 465) |
| en→bg | 53.7 | 1 hop | 62 (p90 109) | 52.8 | 1 hop | 692 (p90 1215) |
| en→fi | 56.4 | 1 hop | 60 (p90 105) | 56.9 | 1 hop | 633 (p90 1089) |
| en→sv | 61.1 | 1 hop | 30 (p90 51) | 58.7 | 1 hop | 234 (p90 391) |
| en→hu | 45.9 | 1 hop | 63 (p90 109) | 46.2 | 1 hop | 642 (p90 1135) |
| en→da | 63.4 | 1 hop | 30 (p90 52) | 61.8 | 1 hop | 262 (p90 457) |
| en→et | 55.2 | 1 hop | 58 (p90 105) | 53.7 | 1 hop | 593 (p90 991) |
| en→lv | 49.9 | 1 hop | 62 (p90 114) | 44.2 | 1 hop | 635 (p90 1089) |
| en→lt | 60.8 | 1 hop | 62 (p90 116) | 58.6 | 1 hop | 655 (p90 1168) |
| en→sl | 59.5 | 1 hop | 62 (p90 112) | 51.7 | 1 hop | 283 (p90 485) |
| en→el | 54.1 | 1 hop | 32 (p90 60) | 54.7 | 1 hop | 715 (p90 1270) |
| en→mt | - | - | - | 58.4 | 1 hop | 294 (p90 471) |

Average over the 53 pairs both have: Mozilla chrF 55.2, 52 ms a sentence (3.5 ms a word), Opus-MT chrF 51.0, 477 ms a sentence (27.7 ms a word)

## between two others

| pair | Mozilla chrF | route | ms/sentence | Opus-MT chrF | route | ms/sentence |
|---|---|---|---|---|---|---|
| de→fr | 48.2 | 2 hops | 122 (p90 204) | 45.1 | 1 hop | 304 (p90 497) |
| fr→de | 45.2 | 2 hops | 121 (p90 208) | 43.8 | 1 hop | 312 (p90 512) |
| es→it | 57.4 | 2 hops | 117 (p90 199) | 57.0 | 1 hop | 279 (p90 459) |
| it→es | 60.4 | 2 hops | 120 (p90 203) | 58.8 | 1 hop | 284 (p90 499) |
| ru→uk | 43.9 | 2 hops | 131 (p90 219) | 41.2 | 1 hop | 270 (p90 452) |
| uk→ru | 42.2 | 2 hops | 93 (p90 172) | 42.1 | 1 hop | 268 (p90 496) |
| pl→cs | 47.0 | 2 hops | 124 (p90 216) | 37.6 | 2 hops | 505 (p90 862) |
| cs→pl | 45.3 | 2 hops | 120 (p90 209) | 38.2 | 2 hops | 551 (p90 926) |
| ja→ko | 21.2 | 2 hops | 98 (p90 154) | 1.9 | 2 hops | 1263 (p90 4457) |
| ko→ja | 28.3 | 2 hops | 100 (p90 171) | 1.7 | 2 hops | 1516 (p90 4287) |
| zh→ja | 27.5 | 2 hops | 109 (p90 191) | 2.2 | 1 hop | 1010 (p90 3266) |
| ja→zh | 18.5 | 2 hops | 96 (p90 152) | 12.5 | 2 hops | 406 (p90 650) |
| de→ja | 29.8 | 2 hops | 115 (p90 198) | 5.3 | 2 hops | 583 (p90 952) |
| ja→de | 42.8 | 2 hops | 104 (p90 161) | 29.8 | 1 hop | 251 (p90 430) |
| ru→es | 47.7 | 2 hops | 131 (p90 222) | 46.9 | 1 hop | 290 (p90 495) |
| es→ru | 41.5 | 2 hops | 123 (p90 205) | 39.5 | 1 hop | 313 (p90 546) |

Average over the 16 pairs both have: Mozilla chrF 40.4, 114 ms a sentence (21.2 ms a word), Opus-MT chrF 31.5, 525 ms a sentence (133.1 ms a word)

