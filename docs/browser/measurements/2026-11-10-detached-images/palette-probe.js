(() => {
 if (!globalThis.__paletteDump && globalThis.__paletteImage) {
  const i=__paletteImage,c=document.createElement('canvas');c.width=c.height=480;
  const x=c.getContext('2d');x.drawImage(i,0,0);
  globalThis.__paletteDump={src:i.src,complete:i.complete,natural:[i.naturalWidth,i.naturalHeight],samples:[[0,0],[100,100],[240,240],[479,479]].map(p=>Array.from(x.getImageData(p[0],p[1],1,1).data))};
 }
 return JSON.stringify({dpr:devicePixelRatio,palette:globalThis.__paletteDump||null,wave:Array.from(document.querySelectorAll('canvas.single-wave__canvas')).map(c=>[c.width,c.height])});
})()
