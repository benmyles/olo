import copy,json,os,socket,subprocess,sys
from pathlib import Path
screen="codex-browser"
out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
def run(*a):return subprocess.check_output(["olo-screen",*map(str,a)],text=True)
def ipc(req):
 run("wait",screen,"--free")
 with socket.socket(socket.AF_UNIX) as s:
  s.connect(os.environ["OLO_SOCKET"]);s.sendall((json.dumps(req)+"\n").encode());return json.loads(s.makefile().readline())
original=ipc({"type":"get_settings"})["settings"];rows=[]
try:
 for mode in ["light","dark","night"]:
  for scale in [1,1.25,1.5,1.75,2]:
   settings=copy.deepcopy(original);settings["appearance"]["theme"]="dark" if mode=="night" else mode;settings["appearance"]["night_light"]["mode"]="on" if mode=="night" else "off";settings["appearance"]["night_light"]["temperature"]=4500;settings["display"]["scale"]=scale
   for output in settings["display"].get("outputs",{}).values():output.pop("scale",None)
   assert ipc({"type":"set_settings","settings":settings})["type"]=="ok"
   w=next(w for w in json.loads(run("windows",screen)) if w["app_id"]=="olo.browser")
   run("place",screen,49,40,"1000x700","--window",w["id"])
   run("wait",screen,"--idle")
   review=json.loads(run("review",screen,"--window",w["id"],"--json"))
   run("look",screen,out/f"{mode}-{scale}.png","--window",w["id"],"--no-pointer")
   rows.append({"mode":mode,"scale":scale,"review":review})
finally:ipc({"type":"set_settings","settings":original})
(out/"review.json").write_text(json.dumps(rows,indent=2))
print(json.dumps(rows))
