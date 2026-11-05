#!/usr/bin/python3
"""Literal data ports only. No JS or color library execution/golden generation."""
from pathlib import Path
import ast,json,re
ROOT=Path(__file__).parent
UNKNOWN=object()
SKIP={'skip': True}
TOKEN=re.compile(r'''\s+|//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|[A-Za-z_$][\w$]*|(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?|.''')
class Data:
 def __init__(self,text):
  self.text=text;self.tokens=[];self.positions=[]
  for m in TOKEN.finditer(text):
   token=m[0]
   if token.isspace() or token.startswith(('//','/*')):continue
   self.tokens.append(token);self.positions.append(m.start())
  self.end={};self.parent={};stack=[]
  for i,t in enumerate(self.tokens):
   if t in ('{','[','('):
    if stack:self.parent[i]=stack[-1]
    stack.append(i)
   elif t in ('}',']',')') and stack:
    start=stack.pop();self.end[start]=i
 def value(self,i,env=None):
  t=self.tokens[i]
  if t.startswith(('"',"'")):return ast.literal_eval(t),i+1
  if t.startswith('`'):
   s=t[1:-1]
   names=re.findall(r'\$\{(\w+)\}',s)
   if names and (not env or any(n not in env for n in names)):return UNKNOWN,i+1
   return re.sub(r'\$\{(\w+)\}',lambda m:str(env[m[1]]),s),i+1
  if t in ('null','undefined','NaN'):return (None if t!='undefined' else SKIP),i+1
  if t in ('true','false'):return t=='true',i+1
  if t=='-' and re.fullmatch(r'[\d.eE+-]+',self.tokens[i+1]):return -float(self.tokens[i+1]),i+2
  if re.fullmatch(r'(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?',t):return float(t),i+1
  if t=='[':
   result=[];i+=1
   while self.tokens[i]!=']':
    if self.tokens[i]==',':result.append(SKIP);i+=1;continue
    value,i=self.value(i,env)
    if value is UNKNOWN:return UNKNOWN,self.end.get(i-1,i)+1
    result.append(value)
    if self.tokens[i]==',':i+=1
    elif self.tokens[i]!=']':return UNKNOWN,i
   return result,i+1
  if t=='{':
   fields={}
   for key,index in self.fields(i).items():fields[key]=self.value(index,env)[0]
   return fields,self.end[i]+1
  if env and t in env:return env[t],i+1
  return UNKNOWN,self.end.get(i,i)+1
 def fields(self,start):
  fields={};i=start+1;end=self.end[start]
  while i<end:
   if i+1<end and self.tokens[i+1]==':':
    key=self.tokens[i];key=ast.literal_eval(key) if key.startswith(('"',"'")) else key
    fields[key]=i+2;i+=2
   if self.tokens[i] in ('{','[','('):i=self.end[i]+1
   else:i+=1
  return fields
 def ancestors(self,i):
  while i in self.parent:
   i=self.parent[i];yield i

def dump(name,value): (ROOT/name).write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n')

# Native literal ports only. Known helper wrappers are expanded from their
# pinned definitions; no upstream JS, CSSOM or engine is executed.
from html.parser import HTMLParser
class ElementContext(HTMLParser):
 def __init__(self):
  super().__init__();self.root={'tag':'body','children':[]};self.stack=[self.root];self.body=False;self.target=None
 def handle_starttag(self,tag,attributes):
  if tag=='body':self.body=True;return
  if not self.body:
   if tag in {'html','head','meta','link','title','script','style'}:return
   self.body=True
  node={'tag':tag,'children':[]};parent=self.stack[-1];parent['children'].append(node)
  if dict(attributes).get('id')=='target':self.target=(parent,node)
  if tag not in {'meta','link','br','img','input','hr','source','wbr'}:self.stack.append(node)
 def handle_endtag(self,tag):
  for i in range(len(self.stack)-1,0,-1):
   if self.stack[i]['tag']==tag:self.stack=self.stack[:i];return
 def result(self):
  if self.target is None:return {}
  parent,node=self.target
  return {'sibling_index':next(i+1 for i,x in enumerate(parent['children']) if x is node),'sibling_count':len(parent['children'])}

cases=[];unported=[]
math_names={'test_math_used','test_math_computed','test_math_specified','test_nan','test_plus_infinity','test_minus_infinity','test_plus_zero','test_minus_zero','test_zero','test_invalid_value','test_invalid_number','test_invalid_angle','test_invalid_length','test_invalid_length_percentage','test_length_equals','test_angle_equals','test_time_equals'}
for path in sorted((ROOT/'wpt').glob('*.html')):
 text=path.read_text();element_context=ElementContext();element_context.feed(text);d=Data(text);loops={};functions=[];wrapper_types={};wrapper_properties={};invalid_functions={};zero_template=None
 # Function bodies are adapters, not independent data calls.
 for i,t in enumerate(d.tokens):
  if t=='function' and i+2<len(d.tokens) and d.tokens[i+2]=='(':
   body=d.end[i+2]+1
   if d.tokens[body]=='{':
    functions.append((body,d.end[body]))
    match=re.search(r"type:\s*['\"](\w+)['\"]",text[d.positions[body]:d.positions[d.end[body]]])
    if match:wrapper_types[d.tokens[i+1]]=match[1]
    props=re.findall(r"test_invalid_value\(['\"]([\w-]+)['\"]",text[d.positions[body]:d.positions[d.end[body]]])
    if props:wrapper_properties[d.tokens[i+1]]=props[0]
    if d.tokens[i+1].startswith('test_invalid_'):invalid_functions[d.tokens[i+1]]=(body,d.end[body],d.tokens[i+3])
    if d.tokens[i+1]=='test_zero':
     first=re.search(r'test_math_used\(\s*`([^`]+)`',text[d.positions[body]:d.positions[d.end[body]]])
     if first:zero_template=first[1]
  if t=='for' and i+1<len(d.tokens) and d.tokens[i+1]=='(':
   end=d.end[i+1];body=end+1
   if d.tokens[body]!='{':continue
   header=d.tokens[i+2:end]
   if len(header)>3 and header[0] in ('let','const') and header[2]=='of':
    values,_=d.value(i+5)
    if isinstance(values,list) and all(isinstance(x,(int,float,str)) for x in values):loops[body]=(header[1],values)
 def invalid_templates(name,seen=()):
  if name not in invalid_functions or name in seen:return []
  start,end,param=invalid_functions[name];out=[]
  for k in range(start+1,end):
   if d.tokens[k]=='test_invalid_value' and d.tokens[k+1]=='(':
    prop,j=d.value(k+2)
    if not isinstance(prop,str) or d.tokens[j]!=',':continue
    token=d.tokens[j+1]
    if token==param:template='${value}'
    elif token.startswith('`'):template=token[1:-1].replace('${'+param+'}','${value}')
    else:continue
    out.append((prop,template))
   elif d.tokens[k].startswith('test_invalid_') and d.tokens[k]!='test_invalid_value' and d.tokens[k+1:k+3]==['(',param]:
    out.extend(invalid_templates(d.tokens[k],seen+(name,)))
  return out
 for i,t in enumerate(d.tokens):
  if t not in math_names or i+1>=len(d.tokens) or d.tokens[i+1]!='(':continue
  if i and d.tokens[i-1]=='function':continue
  if any(a<i<b for a,b in functions):continue
  end=d.end[i+1];environments=[{}]
  for ancestor in reversed(list(d.ancestors(i+1))):
   if ancestor in loops:
    key,values=loops[ancestor];environments=[dict(env,**{key:value}) for env in environments for value in values]
  line=text.count('\n',0,d.positions[i])+1
  for env in environments:
   args=[];j=i+2
   while j<end:
    value,next_index=d.value(j,env);args.append(value)
    if next_index>=end:break
    if d.tokens[next_index]!=',':args=[UNKNOWN];break
    j=next_index+1
   base={'file':path.name,'line':line,'offset':d.positions[i],'helper':t,'environment':env}
   if not args or args[0] is UNKNOWN or not isinstance(args[0],str):unported.append(dict(base,reason='non-literal input/environment'));continue
   options={};expected=None;mode='computed';property=None;source=args[0]
   if t=='test_invalid_value':
    property=source;source=args[1] if len(args)>1 else UNKNOWN;mode='invalid'
   elif t.startswith('test_invalid_'):
    property=wrapper_properties.get(t,{'test_invalid_number':'opacity','test_invalid_angle':'rotate','test_invalid_length':'letter-spacing','test_invalid_length_percentage':'margin-left'}[t]);mode='invalid'
   elif t in ('test_nan','test_plus_infinity','test_minus_infinity','test_plus_zero','test_minus_zero'):
    expected={'test_nan':'calc(NaN)','test_plus_infinity':'calc(infinity)','test_minus_infinity':'calc(-infinity)','test_plus_zero':'calc(infinity)','test_minus_zero':'calc(-infinity)'}[t];options={'type':'number'};mode='raw'
    if t in ('test_plus_zero','test_minus_zero'):source=f'calc(1 / {source})'
   elif t=='test_zero':
    config=args[1] if len(args)>1 else UNKNOWN
    if not isinstance(config,dict) or not isinstance(config.get('is_negative'),bool):unported.append(dict(base,reason='non-literal zero expectation'));continue
    options={'type':config.get('type','integer')}
    # Each local wrapper's first result is zero; the sign assertion below
    # is the independent part that distinguishes +0 and -0.
    zero_first=(zero_template or 'sign(${expression})').replace('${expression}',source)
    source=f'clamp(-1, 1 / sign({source}), 1)';expected='-1' if config['is_negative'] else '1'
   else:
    expected=args[1] if len(args)>1 else UNKNOWN
    options=args[2] if len(args)>2 and isinstance(args[2],dict) else {}
    if t in wrapper_types:options['type']=wrapper_types[t]
    elif t=='test_angle_equals':options['type']='angle'
    elif t=='test_time_equals':options['type']='time'
    elif t=='test_length_equals':options['type']='length'
    if t=='test_math_specified':mode='specified'
    if t=='test_math_computed':mode='computed_serialization'
   if not isinstance(source,str) or (mode!='invalid' and not isinstance(expected,str)):
    unported.append(dict(base,reason='non-literal input/reference'));continue
   options={k:v for k,v in options.items() if k in ('type','prop','approx','extraStyle') and v is not UNKNOWN}
   if property is not None:options['prop']=property
   # Record literal fixture environment. Font/percentage context is a native
   # numeric model, not measured layout or inferred from expected outputs.
   before=text[:d.positions[i]]
   font=re.search(r'font-size:\s*([\d.]+)px',text)
   width=re.search(r'width:\s*([\d.]+)px',text)
   context={'em':float(font[1]) if font else 16.,'rem':10. if re.search(r':root\s*\{\s*font-size:\s*10px',text) else 16.,'width':float(width[1]) if width else 100.}
   context.update(element_context.result())
   line_height=re.search(r'line-height:\s*([\d.]+)px',text)
   if line_height:context['line_height']=float(line_height[1])
   assignments=list(re.finditer(r"getElementById\(['\"]container['\"]\)\.style\.width\s*=\s*['\"]([\d.]+)px['\"]",before))
   if assignments:context['width']=float(assignments[-1][1])
   setters=list(re.finditer(r"container\.style\.(width|fontSize)\s*=\s*['\"]([\d.]+)px['\"]",before))
   for setter in setters:context['width' if setter[1]=='width' else 'em']=float(setter[2])
   if t=='test_zero':cases.append(dict(base,input=zero_first,expected='0',options=options,context=context,mode='computed'))
   if t.startswith('test_invalid_') and t!='test_invalid_value':
    for prop,template in invalid_templates(t) or [(property,'${value}')]:
     value=template.replace('${value}',source);opts=dict(options,prop=prop)
     # A rotate(<angle>) wrapper tests the angle argument. Port that
     # numeric contract explicitly, not transform parsing.
     if template=='rotate(${value})':value=source;opts['prop']='rotate'
     cases.append(dict(base,input=value,expected=None,options=opts,context=context,mode=mode,source_property=prop,driver_template=template))
   else:cases.append(dict(base,input=source,expected=expected,options=options,context=context,mode=mode))
   if t=='test_nan':cases.append(dict(base,input=f'calc(-1 * {source})',expected=expected,options=options,context=context,mode='raw'))
 for i,t in enumerate(d.tokens):
  if t not in {'testComputedValueGreaterOrLowerThan','test_serialization','test_specified_serialization'} or d.tokens[i+1]!='(':continue
  if i and d.tokens[i-1]=='function':continue
  if any(a<i<b for a,b in functions):continue
  unported.append({'file':path.name,'line':text.count('\n',0,d.positions[i])+1,'helper':t,'reason':'property limit/serialization helper not ported'})
dump('derived.json',{'scope':'Native numeric value/type/IEEE ports of literal WPT assertions; not upstream JS, CSSOM serialization, style/layout or full WPT pages','cases':cases,'unported':unported})
print('Numeric literal cases:',len(cases),'unported call forms:',len(unported))
