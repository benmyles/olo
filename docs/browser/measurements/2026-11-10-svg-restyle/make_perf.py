from pathlib import Path
r=Path('/var/tmp/codex-browser-svg-refresh')
shape="<svg width=40 height=24><path d='M0 0H40V24H0Z' fill='red'/></svg>"
body='<!doctype html><title>SVG refresh performance</title><style>html{overflow:scroll}body{margin:20px;height:3000px;background:white;color:black;font:16px sans-serif}.selected path{fill:blue}svg{display:inline-block;margin:2px}#unrelated.selected{color:blue}</style><h1>SVG refresh performance</h1><button id=toggle>Toggle SVG</button><span id=unrelated>Unrelated text</span><div id=chosen>'+shape+'</div><div>'+shape*120+'</div>'
body+='''<script>
const chosen=document.getElementById('chosen'),text=document.getElementById('unrelated');
document.getElementById('toggle').onclick=()=>chosen.classList.toggle('selected');
const results={cached:[],changed:[],unrelated:[],frameTimes:[]};
let started=0,last=0,frames=0;
function animate(t){if(last)results.frameTimes.push(t-last);last=t;chosen.classList.toggle('selected');if(++frames<90){requestAnimationFrame(animate);return}chosen.classList.add('selected');results.done=true;for(const key of ['cached','changed','unrelated','frameTimes'])results[key]=results[key].map(x=>Math.round(x*1000)/1000);console.log('SVG_BENCH '+JSON.stringify(results))}
function round(){let sum=0,t=performance.now();for(let i=0;i<3000;i++)sum+=chosen.getBoundingClientRect().width;results.cached.push(performance.now()-t);t=performance.now();for(let i=0;i<20;i++){chosen.classList.toggle('selected');sum+=chosen.getBoundingClientRect().width}results.changed.push(performance.now()-t);t=performance.now();for(let i=0;i<20;i++){text.classList.toggle('selected');sum+=text.getBoundingClientRect().width}results.unrelated.push(performance.now()-t);if(results.changed.length<7){setTimeout(round,100);return}requestAnimationFrame(animate)}setTimeout(round,800)
</script>'''
(r/'perf.html').write_text(body)
s=Path('docs/browser/measurements/2026-11-10-scroll-into-view/measure.py').read_text().replace('/var/tmp/codex-browser-scroll',str(r)).replace('Scroll geometry performance','SVG refresh performance').replace('SCROLL_BENCH','SVG_BENCH')
s=s.replace("    assert len(bench['destinations'])==7\n    if variant=='after':assert all(row['valid'] for row in bench['destinations']),bench", "    assert bench['done'] and len(bench['changed'])==7,bench\n    run('look',screen,root/f'perf-{scale}-{variant}-{len(rows)}.png','--window',w['id'],'--no-pointer')")
s=s.replace('check.at=(500,350)','check.at=(900,500)')
(r/'measure.py').write_text(s)
