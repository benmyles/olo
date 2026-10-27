<!-- Classes And Methods - The Midi Kit - 
General MIDI Instruments -->

##  General MIDI Instruments

Declared in: `midi/MidiDefs.h`

The General MIDI Specification declares a set of 128 instruments, numbered 0-127. The Midi Kit provides a set of handy midi_axe constants that you can use to identify a MIDI instrument in funtions such as [`BMidiSynth::LoadInstrument()`](BMidiSynth.md#BMidiSynth_LoadInstrument "LoadInstrument\(\), RemapInstrument\(\), UnloadInstrument\(\), FlushInstrumentCache\(\)") and [`BMidi::ProgramChange()`](BMidi.md#BMidi_ProgramChange "ProgramChange\(\), SprayProgramChange\(\)").

#| Pianos  
---|---  
0| `B_ACOUSTIC_GRAND`  
1| `B_BRIGHT_GRAND`  
2| `B_ELECTRIC_GRAND`  
3| `B_HONKY_TONK`  
4| `B_ELECTRIC_PIANO_1`  
5| `B_ELECTRIC_PIANO_2`  
6| `B_HARPSICHORD`  
7| `B_CLAVICHORD`  
  
#| Tuned Idiophones  
---|---  
8| `B_CELESTA`  
9| `B_GLOCKENSPIEL`  
10| `B_MUSIC_BOX`  
11| `B_VIBRAPHONE`  
12| `B_MARIMBA`  
13| `B_XYLOPHONE`  
14| `B_TUBULAR_BELLS`  
15| `B_DULCIMER`  
  
#| Organs  
---|---  
16| `B_DRAWBAR_ORGAN`  
17| `B_PERCUSSIVE_ORGAN`  
18| `B_ROCK_ORGAN`  
19| `B_CHURCH_ORGAN`  
20| `B_REED_ORGAN`  
21| `B_ACCORDION`  
22| `B_HARMONICA`  
23| `B_TANGO_ACCORDION`  
  
#| Guitars  
---|---  
24| `B_ACOUSTIC_GUITAR_NYLON`  
25| `B_ACOUSTIC_GUITAR_STEEL`  
26| `B_ELECTRIC_GUITAR_JAZZ`  
27| `B_ELECTRIC_GUITAR_CLEAN`  
28| `B_ELECTRIC_GUITAR_MUTED`  
29| `B_OVERDRIVEN_GUITAR`  
30| `B_DISTORTION_GUITAR`  
31| `B_GUITAR_HARMONICS`  
  
#| Basses  
---|---  
32| `B_ACOUSTIC_BASS`  
33| `B_ELECTRIC_BASS_FINGER`  
34| `B_ELECTRIC_BASS_PICK`  
35| `B_FRETLESS_BASS`  
36| `B_SLAP_BASS_1`  
37| `B_SLAP_BASS_2`  
38| `B_SYNTH_BASS_1`  
39| `B_SYNTH_BASS_2`  
  
#| Strings and Timpani  
---|---  
40| `B_VIOLIN`  
41| `B_VIOLA`  
12| `B_CELLO`  
43| `B_CONTRABASS`  
44| `B_TREMOLO_STRINGS`  
45| `B_PIZZICATO_STRINGS`  
46| `B_ORCHESTRAL_STRINGS`  
47| `B_TIMPANI`  
  
#| Ensemble Strings and Voices  
---|---  
48| `B_STRING_ENSEMBLE_1`  
49| `B_STRING_ENSEMBLE_2`  
50| `B_SYNTH_STRINGS_1`  
51| `B_SYNTH_STRINGS_2`  
52| `B_VOICE_AAH`  
53| `B_VOICE_OOH`  
54| `B_SYNTH_VOICE`  
55| `B_ORCHESTRA_HIT`  
  
#| Brass  
---|---  
56| `B_TRUMPET`  
57| `B_TROMBONE`  
58| `B_TUBA`  
59| `B_MUTED_TRUMPET`  
60| `B_FRENCH_HORN`  
61| `B_BRASS_SECTION`  
62| `B_SYNTH_BRASS_1`  
63| `B_SYNTH_BRASS_2`  
  
#| Reeds  
---|---  
64| `B_SOPRANO_SAX`  
65| `B_ALTO_SAX`  
66| `B_TENOR_SAX`  
67| `B_BARITONE_SAX`  
68| `B_OBOE`  
69| `B_ENGLISH_HORN`  
70| `B_BASSOON`  
71| `B_CLARINET`  
  
#| Pipes  
---|---  
72| `B_PICCOLO`  
73| `B_FLUTE`  
74| `B_RECORDER`  
75| `B_PAN_FLUTE`  
76| `B_BLOWN_BOTTLE`  
77| `B_SHAKUHACHI`  
78| `B_WHISTLE`  
79| `B_OCARINA`  
  
#| Synth Leads| (Synonyms)  
---|---|---  
80| `B_LEAD_1`| `B_SQUARE_WAVE`  
81| `B_LEAD_2`| `B_SAWTOOTH_WAVE`  
82| `B_LEAD_3`| `B_CALLIOPE`  
83| `B_LEAD_4`| `B_CHIFF`  
84| `B_LEAD_5`| `B_CHARANG`  
85| `B_LEAD_6`| `B_VOICE`  
86| `B_LEAD_7`| `B_FIFTHS`  
87| `B_LEAD_8`| `B_BASS_LEAD`  
  
#| Synth Pads| (Synonyms)  
---|---|---  
88| `B_PAD_1`| `B_NEW_AGE`  
89| `B_PAD_2`| `B_WARM`  
90| `B_PAD_3`| `B_POLYSYNTH`  
91| `B_PAD_4`| `B_CHOIR`  
92| `B_PAD_5`| `B_BOWED`  
93| `B_PAD_6`| `B_METALLIC`  
94| `B_PAD_7`| `B_HALO`  
95| `B_PAD_8`| `B_SWEEP`  
  
#| Musical Effects  
---|---  
96| `B_FX_1`  
97| `B_FX_2`  
98| `B_FX_3`  
99| `B_FX_4`  
100| `B_FX_5`  
101| `B_FX_6`  
102| `B_FX_7`  
103| `B_FX_8`  
  
#| Ethnic  
---|---  
104| `B_SITAR`  
105| `B_BANJO`  
106| `B_SHAMISEN`  
107| `B_KOTO`  
108| `B_KALIMBA`  
109| `B_BAGPIPE`  
110| `B_FIDDLE`  
111| `B_SHANAI`  
  
#| Percussion  
---|---  
112| `B_TINKLE_BELL`  
113| `B_AGOGO`  
114| `B_STEEL_DRUMS`  
115| `B_WOODBLOCK`  
116| `B_TAIKO_DRUMS`  
117| `B_MELODIC_TOM`  
118| `B_SYNTH_DRUM`  
119| `B_REVERSE_CYMBAL`  
  
#| Sound Effects  
---|---  
120| `B_FRET_NOISE`  
121| `B_BREATH_NOISE`  
122| `B_SEASHORE`  
123| `B_BIRD_TWEET`  
124| `B_TELEPHONE`  
125| `B_HELICOPTER`  
126| `B_APPLAUSE`  
127| `B_GUNSHOT`
