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

cases=[];unported=[]
for path in sorted((ROOT/'wpt').rglob('*.html')):
 text=path.read_text();d=Data(text);found=False
 for i,t in enumerate(d.tokens):
  if d.tokens[i+1:i+2]!=['('] or (i and d.tokens[i-1]=='function'):continue
  if t not in ('test_valid_value','test_invalid_value','test_shorthand_value','test_computed_value'):continue
  found=True;loc={'file':str(path.relative_to(ROOT/'wpt')),'line':text.count('\n',0,d.positions[i])+1,'offset':d.positions[i],'helper':t}
  args=[];j=i+2;end=d.end[i+1]
  while j<end:
   value,j=d.value(j);args.append(value)
   if j<end and d.tokens[j]==',':j+=1
   elif j<end:break
  if len(args)<2 or not all(isinstance(v,str) for v in args[:2]):unported.append(dict(loc,reason='nonliteral helper call or loop driver'));continue
  case=dict(loc,property=args[0],input=args[1],mode='parse' if t in ('test_valid_value','test_invalid_value') else 'shorthand' if t=='test_shorthand_value' else 'computed')
  if case['mode']=='parse':
   case['valid']=t=='test_valid_value'
   # Record serialization expectations without treating parsing as CSSOM.
   case['serialization_reference']=args[2] if len(args)>2 and isinstance(args[2],(str,list)) else args[1]
  else:
   expected=args[2] if len(args)>2 else args[1]
   if expected is UNKNOWN or not isinstance(expected,(str,list,dict)):
    unported.append(dict(loc,reason='nonliteral expected value'));continue
   case['expected']=expected
   if len(args)>3:case['requires']='extra JS setup/host arguments'
  cases.append(case)
 if not found:unported.append({'file':str(path.relative_to(ROOT/'wpt')),'helper':'document','reason':'document/JS driver not ported'})
(ROOT/'derived.json').write_text(json.dumps({'scope':'Native declaration grammar, shorthand expansion and isolated numeric/keyword references; not CSSOM serialization, cascade, style, layout, JS or document execution','cases':cases,'unported':unported},ensure_ascii=False,indent=2)+'\n')
print(len(cases),'literal native cases;',len(unported),'unported drivers')
