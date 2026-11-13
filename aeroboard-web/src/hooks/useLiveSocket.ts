import { useEffect,useRef,type RefObject } from 'react';
interface Callbacks {
  binary:(data:ArrayBuffer)=>void;
  control:(message:{type?:string;skipped?:number;runs?:string[];id?:string;kind?:string})=>void;
  connected:(connected:boolean)=>void;
  reconnect:()=>void;
}
/** Own socket reconnects and subscriptions independently of data stores and loaders. */
export function useLiveSocket(url:string,socket:RefObject<WebSocket|null>,runs:RefObject<string[]>,callbacks:Callbacks) {
  const handlers=useRef(callbacks);handlers.current=callbacks;
  useEffect(()=>{
    let disposed=false,attempts=0,hasConnected=false;
    let current:WebSocket|null=null,timer:ReturnType<typeof setTimeout>|undefined;
    const connect=()=>{
      if(disposed)return;
      const ws=new WebSocket(url);current=ws;socket.current=ws;ws.binaryType='arraybuffer';
      ws.onopen=()=>{
        ws.send(JSON.stringify({type:'subscribe',runs:runs.current}));
        window.dispatchEvent(new Event('aeroboard:catalog'));handlers.current.connected(true);attempts=0;
        if(hasConnected)handlers.current.reconnect();hasConnected=true;
      };
      ws.onclose=()=>{
        handlers.current.connected(false);
        if(!disposed)timer=setTimeout(connect,Math.min(30000,1000*2**attempts++));
      };
      ws.onerror=event=>console.error('[Aeroboard] WebSocket error:',event);
      ws.onmessage=event=>{
        try{
          if(event.data instanceof ArrayBuffer)handlers.current.binary(event.data);
          else if(typeof event.data==='string')handlers.current.control(JSON.parse(event.data));
        }catch(error){console.error('[Aeroboard] Failed to handle live message:',error);}
      };
    };
    connect();
    return ()=>{disposed=true;clearTimeout(timer);socket.current=null;current?.close();};
  },[url,socket,runs]);
}
