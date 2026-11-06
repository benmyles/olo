#!/usr/bin/python3
"""Independent native HarfBuzz glyph references; never imports or runs Olo."""
from pathlib import Path
import json,hashlib
import sys
sys.dont_write_bytecode=True
import uharfbuzz as hb
import native_harfbuzz as native
import fontTools
from fontTools.ttLib import TTFont
assert fontTools.__version__=="4.60.1"
assert hb.__version__=='0.55.0'
assert hb.version_string()=='14.2.1'
ROOT=Path(__file__).resolve().parent
REPO=ROOT.parents[3]
fonts={
 'latin':REPO/'data/fonts/NotoSans[wght].ttf',
 'arabic':ROOT/'fonts/notosansarabic/NotoSansArabic[wdth,wght].ttf',
 'hebrew':ROOT/'fonts/notosanshebrew/NotoSansHebrew[wdth,wght].ttf',
 'deva':ROOT/'fonts/notosansdevanagari/NotoSansDevanagari[wdth,wght].ttf',
 'thai':ROOT/'fonts/notosansthai/NotoSansThai[wdth,wght].ttf',
 'optical':ROOT.parent/'font-native-fixtures/fonts/optical.ttf',
 'collection':ROOT.parent/'font-native-fixtures/fonts/collection.ttc',
}
cases=[]
def case(name,font,text,script,direction='ltr',language='und',variations=None,features=None,start=0,end=None,index=0):
 # CSS normal requests weight 400, even when a font file defaults to 100.
 variations={'wght':400,'wdth':100,**(variations or {})}
 data=fonts[font].read_bytes();face=hb.Face(data,index);hfont=hb.Font(face);hfont.scale=(face.upem,face.upem);hb.ot_font_set_funcs(hfont)
 if variations:hfont.set_variations(variations)
 encoded=text.encode();end=len(encoded) if end is None else end
 buffer=hb.Buffer();buffer.add_utf8(encoded,start,end-start);buffer.script=script;buffer.direction=direction;buffer.language=language;buffer.cluster_level=hb.BufferClusterLevel.MONOTONE_GRAPHEMES;buffer.flags=(hb.BufferFlags.BOT if start==0 else 0)|(hb.BufferFlags.EOT if end==len(encoded) else 0)
 hb.shape(hfont,buffer,features or {})
 upem,rows_native=native.shape(data,text,start,end,script,direction,language,variations,features or {},index)
 font_info=TTFont(fonts[font],fontNumber=index);family=font_info['name'].getDebugName(16) or font_info['name'].getDebugName(1)
 rows=[dict(id=i.codepoint,cluster=i.cluster,x_advance=p.x_advance,y_advance=p.y_advance,x_offset=p.x_offset,y_offset=p.y_offset,unsafe_to_break=bool(i.flags&hb.GlyphFlags.UNSAFE_TO_BREAK)) for i,p in zip(buffer.glyph_infos,buffer.glyph_positions)]
 cases.append(dict(name=name,collection_index=index,font=str(fonts[font].relative_to(REPO)),family=family,sha256=hashlib.sha256(data).hexdigest(),text=text,start=start,end=end,script=script,direction=direction,language=language,variations=variations or {},features=features or {},upem=upem,glyphs=rows_native,older_harfbuzz_14_glyphs=rows))
case('latin-kern-ligatures','latin','office AVATAR affine','Latn',language='en')
case('latin-no-liga','latin','office affine','Latn',language='en',features={'liga':False})
case('latin-no-kern','latin','AVATAR To','Latn',features={'kern':False})
case('latin-marks','latin','e\u0301 a\u0323\u0302 Å ﬁ','Latn')
case('greek','latin','ἄνθρωπος Ελληνικά','Grek',language='el')
case('cyrillic','latin','Русский язык','Cyrl',language='ru')
case('variable-weight','latin','office AV','Latn',variations={'wght':650.5})
case('arabic','arabic','السلام عليكم','Arab','rtl','ar')
case('arabic-marks','arabic','السَّلَامُ عَلَيْكُمْ','Arab','rtl','ar')
case('arabic-joiners','arabic','لا ل\u200dا ل\u200cا','Arab','rtl','ar')
case('arabic-context-slice','arabic','سلام','Arab','rtl','ar',start=len('س'.encode()),end=len('سلا'.encode()))
case('arabic-variable','arabic','مرحبا بالعالم','Arab','rtl','ar',variations={'wght':650.5,'wdth':87.5})
case('hebrew','hebrew','שלום עוֹלָם','Hebr','rtl','he')
case('hebrew-mirroring','hebrew','(שָׁלוֹם)','Hebr','rtl','he')
case('deva-conjuncts','deva','क्षेत्र हिन्दी','Deva','ltr','hi')
case('deva-joiners','deva','क्\u200dष क्\u200cष','Deva','ltr','hi')
case('deva-variable','deva','नमस्ते भारत','Deva','ltr','hi',variations={'wght':650.5,'wdth':87.5})
case('thai','thai','ภาษาไทย สวัสดี','Thai','ltr','th')
case('thai-marks','thai','น้ำ เก้า ป่า','Thai','ltr','th')
case('vertical-primitive','latin','ABC','Latn','ttb')
case('optical-20','optical','0xH','Latn',variations={'opsz':20})
case('absolute-utf8-clusters','latin','é office Ω','Latn',start=len('é '.encode()),end=len('é office'.encode()))
case('collection-first','collection','0xH','Latn',index=0)
case('collection-second','collection','0水','Hani',index=1)
authored=cases.copy();cases=[]
for key,unit,script,direction,language in [('latin','office AVATAR affine ','Latn','ltr','en'),('arabic','السَّلَامُ عَلَيْكُمْ ','Arab','rtl','ar'),('hebrew','שלום עוֹלָם ','Hebr','rtl','he'),('deva','क्षेत्र हिन्दी नमस्ते ','Deva','ltr','hi'),('thai','ภาษาไทย สวัสดี ','Thai','ltr','th')]:
 for target in [128,1024,4096]:
  text=unit*max(1,target//len(unit.encode()))
  case(key+'-'+str(target),key,text,script,direction,language)
import struct
for row in cases:
 rows=row.pop('glyphs');row.pop('older_harfbuzz_14_glyphs')
 row['glyph_count']=len(rows);row['x_advance']=sum(g['x_advance']for g in rows);row['y_advance']=sum(g['y_advance']for g in rows)
 payload=b''.join(struct.pack('<IIiiiiB',g['id'],g['cluster'],g['x_advance'],g['y_advance'],g['x_offset'],g['y_offset'],g['unsafe_to_break'])for g in rows)
 row['glyph_sha256']=hashlib.sha256(payload).hexdigest()
benchmarks=cases;cases=authored
(ROOT/'harfbuzz.json').write_text(json.dumps(dict(oracle='native HarfBuzz C API, with older uharfbuzz reference retained separately',uharfbuzz=hb.__version__,older_harfbuzz=hb.version_string(),harfbuzz=native.version,scope='Explicit direction/script/language/variations/features and UTF-8 byte clusters; no Olo outputs, automatic font selection, bidi, line breaking or screenshots consulted.',cases=cases,benchmarks=benchmarks),ensure_ascii=False,indent=2)+'\n')
print('Wrote',len(cases),'independent native HarfBuzz cases')
