(()=>{
 const state=globalThis.__glTrace={contexts:[],sources:[]};const contexts=new WeakMap(),locs=new WeakMap();
 function context(g){if(!contexts.has(g)){let c={canvas:g.canvas&&g.canvas.className,uniforms:{},textures:[]};contexts.set(g,c);state.contexts.push(c);}return contexts.get(g);}
 for(const C of [globalThis.WebGLRenderingContext,globalThis.WebGL2RenderingContext]){if(!C)continue;let p=C.prototype;
 for(const name of Object.getOwnPropertyNames(p)){
  const d=Object.getOwnPropertyDescriptor(p,name);if(typeof d.value!=='function')continue;const f=d.value;
  if(name==='getUniformLocation')p[name]=function(program,n){let l=f.apply(this,arguments);if(l)locs.set(l,String(n));return l};
  else if(name==='shaderSource')p[name]=function(shader,source){if(state.sources.length<100)state.sources.push({canvas:this.canvas&&this.canvas.className,source:String(source)});return f.apply(this,arguments)};
  else if(/^uniform[1234]/.test(name)||/^uniformMatrix/.test(name))p[name]=function(l,...args){let n=locs.get(l);if(n){context(this).uniforms[n]={method:name,args:args.map(v=>ArrayBuffer.isView(v)?Array.from(v).slice(0,100):v)};}return f.call(this,l,...args)};
  else if(/^(texImage|texSubImage|texStorage)/.test(name))p[name]=function(...args){let source=args[args.length-1];if(this.canvas&&this.canvas.className==='single-wave__canvas'&&source instanceof HTMLImageElement)globalThis.__paletteImage=source;let c=context(this);if(c.textures.length<100)c.textures.push({method:name,args:args.map(v=>ArrayBuffer.isView(v)?{length:v.length,head:Array.from(v).slice(0,16)}:typeof v==='object'&&v?{kind:Object.prototype.toString.call(v),width:v.width,height:v.height}:v)});return f.apply(this,args)};
 }
 }
 return 'capture installed';
})()
