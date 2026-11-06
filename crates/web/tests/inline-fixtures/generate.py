#!/usr/bin/python3
"""Original adversarial fonts from literal geometry/GSUB; no Olo execution.

fontTools 4.60.1, isolated interpreter under /var/tmp. Font timestamps fixed.
"""
from pathlib import Path
import hashlib,json
import fontTools
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
assert fontTools.__version__=='4.60.1'
ROOT=Path(__file__).parent
glyphs=['.notdef','space','A','B','C','D','E','f','i','ligAB','fi']
for name,features in [('plain',''),('ligature','feature liga {sub A space B by ligAB;sub f i by fi;} liga;'),('deletion',"feature calt {sub D' space E by NULL;} calt;")]:
 fb=FontBuilder(1000,isTTF=True);fb.setupGlyphOrder(glyphs);fb.setupCharacterMap({32:'space',**{ord(c):c for c in 'ABCDEfi'}})
 outlines={};advances={}
 for glyph in glyphs:
  pen=TTGlyphPen(None)
  if glyph!='space':pen.moveTo((0,0));pen.lineTo((400,0));pen.lineTo((400,700));pen.lineTo((0,700));pen.closePath()
  outlines[glyph]=pen.glyph();advances[glyph]=(500 if glyph in ('space','ligAB')else 900 if glyph=='fi'else 1000,0)
 fb.setupGlyf(outlines);fb.setupHorizontalMetrics(advances);fb.setupHorizontalHeader(ascent=800,descent=-200,lineGap=0)
 fb.setupNameTable(dict(familyName='Olo Inline '+name,styleName='Regular',uniqueFontIdentifier='OloInline-'+name,fullName='Olo Inline '+name,psName='OloInline-'+name))
 fb.setupOS2(sTypoAscender=800,sTypoDescender=-200,sTypoLineGap=0,usWinAscent=800,usWinDescent=200,usWeightClass=400,usWidthClass=5,sxHeight=500,sCapHeight=700,fsSelection=192)
 fb.font['OS/2'].version=4;fb.setupPost();fb.setupMaxp();fb.font['head'].created=3406620150;fb.font['head'].modified=3406620150;fb.font.recalcTimestamp=False
 if features:addOpenTypeFeaturesFromString(fb.font,features)
 fb.save(ROOT/(name+'.ttf'))
print(json.dumps({name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest()for name in ['plain.ttf','ligature.ttf','deletion.ttf']}))
