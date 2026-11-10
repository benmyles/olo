from PIL import Image,ImageDraw
from pathlib import Path
r=Path('/var/tmp/codex-browser-scroll')
for mode in ['light','dark','night']:
 canvas=Image.new('RGB',(1800,1120),'#888');d=ImageDraw.Draw(canvas)
 for col,scale in enumerate([1,1.25,1.5,1.75,2]):
  for row,(v,e)in enumerate([('before','native'),('before','chrome'),('after','native'),('after','chrome')]):
   image=Image.open(r/v/f'{mode}-{scale}-{e}.png').convert('RGB');image.thumbnail((355,250));canvas.paste(image,(col*360,row*280+25));d.text((col*360+4,row*280+4),f'{mode} {scale} {v} {e}',fill='white')
 canvas.save(r/(mode+'-contact.png'))
