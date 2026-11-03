export function downloadText(content: string, name: string, mime: string) {
  const url = URL.createObjectURL(new Blob([content], {type: mime}));
  const link = document.createElement("a"); link.href = url; link.download = name; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function exportChart(card: HTMLElement | null, name: string) {
  const source = card?.querySelector("canvas");
  if (!source) return;
  const canvas = document.createElement("canvas");
  const dpr = window.devicePixelRatio || 1;
  const legend = Array.from(card?.querySelectorAll(".legend-item") ?? []).map(node => ({
    text: `${node.querySelector(".legend-label")?.textContent ?? ""}: ${node.querySelector(".legend-val")?.textContent ?? ""}`,
    color: getComputedStyle(node.querySelector(".legend-val") ?? node).color,
  }));
  canvas.width = source.width; canvas.height = source.height + (50 + legend.length * 20) * dpr;
  const context = canvas.getContext("2d"); if (!context) return;
  context.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--bg-primary").trim() || "#10151c";
  context.fillRect(0,0,canvas.width,canvas.height);
  context.fillStyle = getComputedStyle(card!).color; context.font = `${14*dpr}px system-ui`;
  context.fillText(name,12*dpr,22*dpr);
  legend.forEach((line,index) => {
    context.fillStyle = line.color;context.fillRect(12*dpr,(34+20*index)*dpr,10*dpr,10*dpr);
    context.fillStyle = getComputedStyle(card!).color;context.fillText(line.text,30*dpr,(44+20*index)*dpr);
  });
  context.drawImage(source,0,(50+legend.length*20)*dpr);
  const link = document.createElement("a"); link.download = `${name.replaceAll("/","-")}.png`; link.href = canvas.toDataURL("image/png"); link.click();
}
