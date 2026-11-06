"""Independent C API oracle. No Olo imports or executable are consulted."""
import ctypes as C
import ctypes.util
import os
lib=C.CDLL(os.environ.get('OLO_SHAPING_HARFBUZZ_LIBRARY') or ctypes.util.find_library('harfbuzz'))
P=C.c_void_p;U=C.c_uint32;I=C.c_int32
class Info(C.Structure):_fields_=[('codepoint',U),('mask',U),('cluster',U),('var1',U),('var2',U)]
class Position(C.Structure):_fields_=[('x_advance',I),('y_advance',I),('x_offset',I),('y_offset',I),('var',U)]
class Variation(C.Structure):_fields_=[('tag',U),('value',C.c_float)]
class Feature(C.Structure):_fields_=[('tag',U),('value',U),('start',U),('end',U)]
def bind(name,result,args):
 f=getattr(lib,'hb_'+name);f.restype=result;f.argtypes=args;return f
version=bind('version_string',C.c_char_p,[])().decode()
assert version=='15.0.0',version
blob_new=bind('blob_create',P,[C.c_char_p,U,C.c_int,P,P]);blob_drop=bind('blob_destroy',None,[P])
face_new=bind('face_create',P,[P,U]);face_drop=bind('face_destroy',None,[P]);upem_of=bind('face_get_upem',U,[P])
font_new=bind('font_create',P,[P]);font_drop=bind('font_destroy',None,[P]);ot_funcs=bind('ot_font_set_funcs',None,[P]);scale=bind('font_set_scale',None,[P,I,I]);variations_set=bind('font_set_variations',None,[P,C.POINTER(Variation),U])
buf_new=bind('buffer_create',P,[]);buf_drop=bind('buffer_destroy',None,[P]);add=bind('buffer_add_utf8',None,[P,C.c_char_p,C.c_int,U,C.c_int]);cluster_level=bind('buffer_set_cluster_level',None,[P,C.c_int]);flags=bind('buffer_set_flags',None,[P,U]);direction_set=bind('buffer_set_direction',None,[P,C.c_int]);script_set=bind('buffer_set_script',None,[P,U]);language_set=bind('buffer_set_language',None,[P,P]);direction_parse=bind('direction_from_string',C.c_int,[C.c_char_p,C.c_int]);script_parse=bind('script_from_string',U,[C.c_char_p,C.c_int]);language_parse=bind('language_from_string',P,[C.c_char_p,C.c_int])
shape_fn=bind('shape',None,[P,P,C.POINTER(Feature),U]);success=bind('buffer_allocation_successful',C.c_int,[P]);infos=bind('buffer_get_glyph_infos',C.POINTER(Info),[P,C.POINTER(U)]);positions=bind('buffer_get_glyph_positions',C.POINTER(Position),[P,C.POINTER(U)]);glyph_flags=bind('glyph_info_get_glyph_flags',U,[C.POINTER(Info)])
def shape(data,text,start,end,script,direction,language,variations,features,index=0):
 blob=blob_new(data,len(data),0,None,None);face=face_new(blob,index);font=font_new(face);buffer=buf_new()
 try:
  upem=upem_of(face);ot_funcs(font);scale(font,upem,upem)
  axes=(Variation*len(variations))(*(Variation(int.from_bytes(k.encode(),'big'),v) for k,v in variations.items()));variations_set(font,axes,len(axes))
  encoded=text.encode();add(buffer,encoded,len(encoded),start,end-start);script_set(buffer,script_parse(script.encode(),-1));direction_set(buffer,direction_parse(direction.encode(),-1));language_set(buffer,language_parse(language.encode(),-1));cluster_level(buffer,0);flags(buffer,(1 if start==0 else 0)|(2 if end==len(encoded)else 0))
  fs=(Feature*len(features))(*(Feature(int.from_bytes(k.encode(),'big'),int(v),0,0xffffffff)for k,v in features.items()));shape_fn(font,buffer,fs,len(fs));assert success(buffer)
  count=U();gs=infos(buffer,C.byref(count));ps=positions(buffer,C.byref(count));rows=[]
  for i in range(count.value):
   g,p=gs[i],ps[i];rows.append(dict(id=g.codepoint,cluster=g.cluster,x_advance=p.x_advance,y_advance=p.y_advance,x_offset=p.x_offset,y_offset=p.y_offset,unsafe_to_break=bool(glyph_flags(C.byref(g))&1)))
  return upem,rows
 finally:buf_drop(buffer);font_drop(font);face_drop(face);blob_drop(blob)
