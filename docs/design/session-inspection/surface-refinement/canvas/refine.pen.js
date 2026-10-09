SetVariables({done:{type:'color',value:[{theme:{mode:'light'},value:'#8a918d'},{theme:{mode:'dark'},value:'#7f8783'}]},work:{type:'color',value:[{theme:{mode:'light'},value:'#1d6db0'},{theme:{mode:'dark'},value:'#66b0ef'}]}});
const pos=FindEmptySpace({width:696,height:140,direction:'top',padding:80});
linkedSession=Insert(document,{type:'frame',name:'Linked session · existing contents with open perimeter',x:pos.x,y:pos.y,width:696,layout:'vertical',gap:3,padding:[12,14],stroke:'$line',strokeWidth:{top:1,left:1},fill:'$ground',reusable:true,placeholder:true});
const text=(parent,name,content,props={})=>Insert(parent,{type:'text',name,content,fontFamily:'$sans',fontSize:13,fill:'$ink',textGrowth:'fixed-width',width:'fill_container',lineHeight:1.45,...props});
const head=Insert(linkedSession,{type:'frame',name:'Existing name, state and navigation affordance',width:'fill_container',gap:8,alignItems:'center'});
linkedName=text(head,'Open child session','feat/offline-sync',{fontSize:14,fontWeight:'600'});
linkedDot=Insert(head,{type:'ellipse',name:'Recorded state dot',width:8,height:8,fill:'$done'});
linkedState=Insert(head,{type:'text',name:'Recorded state word',content:'Done',fontFamily:'$sans',fontSize:12,fill:'$muted'});
Insert(head,{type:'icon',name:'Child session navigation',library:'lucide',icon:'chevron-right',width:14,height:14,fill:'$muted'});
linkedMeta=text(linkedSession,'Existing run metadata','Codex run · luna · 1d 0h · 3 steps · $0.23',{fontSize:12,fill:'$muted'});
const briefFrame=Insert(linkedSession,{type:'frame',name:'Existing retained brief',width:'fill_container',padding:[4,0,0,0]});
linkedBrief=text(briefFrame,'Retained task context','Implement the offline queue flush (#212): flush oldest first, stop at the first Nack. Acceptance: the new flush_* tests pass.');
linkedResult=text(linkedSession,'Current activity or recorded result','',{enabled:false,fill:'$muted'});
text(linkedSession,'Existing separate trace action','Run view');
Update(linkedSession,{placeholder:false});
const p=FindEmptySpace({width:1180,height:760,direction:'right',padding:80,nodeId:'vGVHj'});
linkedBoard=Insert(document,{type:'frame',name:'Viewer · restrained linked-session surface states',x:p.x,y:p.y,width:1180,height:760,layout:'vertical',padding:24,gap:20,fill:'$ground',clip:true,placeholder:true});
text(linkedBoard,'State board heading','Keep the viewer; lighten repeated enclosures',{fontSize:16,fontWeight:'600'});
text(linkedBoard,'Scope and source','Existing messages, thinking, grouped tools, event rows, metadata and composer remain in place. Only the child-session perimeter changes; real before/after screenshots are the geometry reference.',{fill:'$muted'});
const cols=Insert(linkedBoard,{type:'frame',name:'Desktop and constrained-width states',width:'fill_container',gap:24});
const left=Insert(cols,{type:'frame',name:'Original reading-width session context',width:696,layout:'vertical',gap:14});
const right=Insert(cols,{type:'frame',name:'Constrained-width dark context',width:'fill_container',layout:'vertical',gap:14,theme:{mode:'dark'},fill:'$ground',padding:16});
for(const parent of [left,right]) for(const state of ['Done','Working','Failed']) {
  const overrides={};
  if(state === 'Working'){overrides[linkedName]={content:'Review offline-sync diff'};overrides[linkedDot]={fill:'#00000000',stroke:'$work',strokeWidth:1.5};overrides[linkedState]={content:'Working'};overrides[linkedMeta]={content:'Subagent · sonnet 5 · 9m · 2 steps · $0.61'};overrides[linkedBrief]={content:"Review Codex's diff on feat/offline-sync as it lands: ordering, Nack handling, no commit before Ack."};overrides[linkedResult]={enabled:true,content:'Reading crates/sync/src/flush.rs'};}
  if(state === 'Failed'){overrides[linkedName]={content:'Check retry safety'};overrides[linkedDot]={fill:'$error'};overrides[linkedState]={content:'Failed',fill:'$error'};overrides[linkedMeta]={content:'Subagent · sonnet 5 · 3m · 1 step · $0.26'};overrides[linkedBrief]={content:'Check whether the retry cap can overflow under repeated Nacks, then report the smallest reproducer.'};overrides[linkedResult]={enabled:true,content:'Result: Failed before producing a reproducer: the review sandbox could not read the test fixture.',fill:'$error'};}
  Insert(parent,{type:'ref',name:state+' linked session · preserved actions and content',ref:linkedSession,width:'fill_container',descendants:overrides});
}
text(linkedBoard,'Fidelity boundary','Component/state board, not a replacement viewer layout. Canvas tooltips and code-span rendering are schematic; authored transcript components and native navigation remain authoritative.',{fill:'$muted'});
Update(linkedBoard,{placeholder:false});
Update(linkedResult,{width:668}); Get(linkedBoard,n=>n.type === 'ref' && /^(Working|Failed)/.test(n.name) && Update(n.id,{descendants:{[linkedResult]:{width:'fill_container'}}})); Print(linkedBoard); Get(linkedBoard,(n,c)=>c.problems&&Print(n.name,c.problems)); Export([linkedBoard],'png','/workspace/semon/docs/design/session-inspection/surface-refinement/canvas',{scale:2});
