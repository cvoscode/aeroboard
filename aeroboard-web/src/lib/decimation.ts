/** Preserve endpoints, extrema, and gaps while bounding canvas work by pixel width. */
export function minMaxIndices(length:number, value:(index:number)=>number, maxPoints:number):number[] {
  if(length<=maxPoints)return Array.from({length},(_,i)=>i);
  const selected=[0],buckets=Math.max(1,Math.floor((maxPoints-2)/2));
  for(let bucket=0;bucket<buckets;bucket++) {
    const start=1+Math.floor((length-2)*bucket/buckets),end=1+Math.floor((length-2)*(bucket+1)/buckets);
    let min=start,max=start,gap=-1;
    for(let i=start;i<end;i++){const v=value(i);if(!Number.isFinite(v)){gap=i;continue;}if(!Number.isFinite(value(min))||v<value(min))min=i;if(!Number.isFinite(value(max))||v>value(max))max=i;}
    if(gap>=0)selected.push(gap);
    selected.push(min);if(max!==min)selected.push(max);
  }
  selected.push(length-1);return Array.from(new Set(selected)).sort((a,b)=>a-b);
}
