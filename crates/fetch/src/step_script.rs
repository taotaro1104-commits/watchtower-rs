//! ブラウザ操作ステップを、ページ内で評価する JavaScript の式にする (純粋関数)。
//!
//! セレクタと入力文字列は、必ず JSON 文字列リテラルとして埋め込む (式の構造を変えられないように)。
//! 式は "ok" / "missing" (要素なし) / "invalid" (セレクタが不正) のいずれかの文字列を返す。

use watchtower_core::BrowserStep;

fn lit(s: &str) -> String {
    serde_json::to_string(s).expect("a string always serializes")
}

/// `Click` / `Type` / `WaitFor` を 1 回評価する式。`Wait` は式を持たない。
pub(crate) fn expression(step: &BrowserStep) -> Option<String> {
    let body = match step {
        BrowserStep::Click { .. } => "e.click();".to_string(),
        BrowserStep::Type { text, .. } => format!(
            "e.focus();\
             const d=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),\"value\");\
             if(d&&d.set){{d.set.call(e,{});}}else{{e.value={};}}\
             e.dispatchEvent(new Event(\"input\",{{bubbles:true}}));\
             e.dispatchEvent(new Event(\"change\",{{bubbles:true}}));",
            lit(text),
            lit(text),
        ),
        BrowserStep::WaitFor { .. } => String::new(),
        BrowserStep::Wait { .. } => return None,
    };
    let selector = match step {
        BrowserStep::Click { selector }
        | BrowserStep::Type { selector, .. }
        | BrowserStep::WaitFor { selector, .. } => selector,
        BrowserStep::Wait { .. } => return None,
    };
    Some(format!(
        "(()=>{{try{{const e=document.querySelector({});if(!e)return \"missing\";{body}return \"ok\";}}catch(x){{return \"invalid\";}}}})()",
        lit(selector),
    ))
}

/// ページ内の要素の位置と、それを一意に指す CSS セレクタを集める式 (ビジュアルセレクタ用)。
/// 結果は JSON 文字列。ページが作る値なので、受け取った側で必ず検証する (`watchtower_core::pick`)。
pub(crate) const PICK_SCRIPT: &str = r#"/*pick*/(()=>{
const W=Math.max(document.documentElement.scrollWidth,innerWidth);
const H=Math.max(document.documentElement.scrollHeight,innerHeight);
const uniq=(s)=>{try{return document.querySelectorAll(s).length===1;}catch(x){return false;}};
const path=(e)=>{
  const parts=[];let n=e;
  while(n&&n.nodeType===1&&n!==document.documentElement){
    if(n.id){const s='#'+CSS.escape(n.id);if(uniq(s)){parts.unshift(s);break;}}
    let i=1,sib=n;while((sib=sib.previousElementSibling)){if(sib.tagName===n.tagName)i++;}
    parts.unshift(n.tagName.toLowerCase()+':nth-of-type('+i+')');
    n=n.parentElement;
  }
  return parts.join(' > ');
};
const skip=['SCRIPT','STYLE','NOSCRIPT','META','LINK','HEAD','BR','TEMPLATE','svg','path'];
const out=[];
const all=document.body?document.body.querySelectorAll('*'):[];
for(const e of all){
  if(out.length>=1500)break;
  if(skip.includes(e.tagName))continue;
  const r=e.getBoundingClientRect();
  if(r.width<4||r.height<4)continue;
  const cs=getComputedStyle(e);
  if(cs.visibility==='hidden'||cs.display==='none')continue;
  out.push({s:path(e),x:r.left+scrollX,y:r.top+scrollY,w:r.width,h:r.height,t:(e.innerText||'').trim().slice(0,80)});
}
return JSON.stringify({w:W,h:H,els:out});
})()"#;
