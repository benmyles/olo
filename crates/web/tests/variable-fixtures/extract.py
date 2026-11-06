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
   return re.sub(r'\$\{(\w+)\}',lambda m:env[m[1]],s),i+1
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
cases=[];groups=[];inventory=[]
for path in sorted((ROOT/'wpt').glob('*.html')):
 text=path.read_text();d=Data(text);ported=False
 def location(i,helper):return {'file':path.name,'line':text.count('\n',0,d.positions[i])+1,'offset':d.positions[i],'helper':helper}
 for i,t in enumerate(d.tokens):
  if d.tokens[i+1:i+2]!=['('] or (i and d.tokens[i-1]=='function'):continue
  if t in ('test_valid_value','test_invalid_value','assert_allowed_variable_value','assert_disallowed_balanced_variable_value'):
   args=[];j=i+2;end=d.end[i+1]
   while j<end:
    value,j=d.value(j);args.append(value)
    if j<end and d.tokens[j]==',':j+=1
    elif j<end:break
   source=args[1] if t.startswith('test_') and len(args)>1 else args[0] if args else UNKNOWN
   if not isinstance(source,str):inventory.append(dict(location(i,t),reason='nonliteral parsing call'));continue
   cases.append(dict(location(i,t),mode='parse',input=source,valid=t in ('test_valid_value','assert_allowed_variable_value')));ported=True
  elif t=='test_cycles':
   args=[];j=i+2;end=d.end[i+1]
   for n in range(4):
    value,j=d.value(j);args.append(value)
    if j<end and d.tokens[j]==',':j+=1
   if not all(isinstance(x,list) and all(isinstance(y,str) for y in x) for x in args[:3]):
    inventory.append(dict(location(i,t),reason='nonliteral cycle group'));continue
   queries=[{'property':p,'invalid':True} for p in args[1]]+[{'property':p,'invalid':False} for p in args[2]]+[{'property':'--sanity','invalid':False}]
   groups.append(dict(location(i,t),declarations=args[0]+['--sanity:valid'],queries=queries,description=args[3]));ported=True
  elif t=='assert_equals' and d.tokens[i+2:i+4]==['computed','('] and path.name=='variable-reference-name-substitution.html':
   declaration,j=d.value(i+4)
   if not isinstance(declaration,str):inventory.append(dict(location(i,t),reason='nonliteral computed declaration'));continue
   computed_end=d.end[i+3];property='width'
   if j<computed_end and d.tokens[j]==',':property,_=d.value(j+1)
   expected,k=d.value(computed_end+2)
   invalid=d.tokens[computed_end+2]=='initialWidth'
   if not isinstance(expected,str) and not invalid:inventory.append(dict(location(i,t),reason='nonliteral computed reference'));continue
   base=re.search(r'#target\s*\{([^}]*)\}',text)
   cases.append(dict(location(i,t),mode='substitute',declarations=declaration,base=base[1] if base else '',property=property,expected=None if invalid else expected,invalid=invalid,requires=['registered_properties'] if '--registered-' in declaration else []));ported=True
 if not ported:
  count=0
  for i,t in enumerate(d.tokens):
   if t in ('test','promise_test') and d.tokens[i+1:i+2]==['(']:
    inventory.append(dict(location(i,t),reason='JS/CSSOM/style/layout driver not ported'));count+=1
  if not count:inventory.append({'file':path.name,'helper':'document','reason':'document/reftest driver not ported'})
dump('derived.json',{'scope':'Native ports of literal var argument/declaration grammar, provided winning custom-property snapshots and literal computed-token references; not WPT JS/CSSOM/cascade/layout/page execution','cases':cases,'cycles':groups,'unported':inventory})
print('Native cases:',len(cases),'cycle groups:',len(groups),'cycle assertions:',sum(len(g['queries']) for g in groups),'unported drivers:',len(inventory))
