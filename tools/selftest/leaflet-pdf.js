// Leaflet opened with pages.pdf: five pages shown, page 1's text found, turned and saved as a new file.
__t.run('leaflet-pdf', async () => {
  const L = await __t.until('the page', () => window.Leaflet);
  const seen = [];
  const handle = L.handle.bind(L);
  L.handle = (m) => { seen.push(m.what); return handle(m); };
  await __t.until('the PDF', () => L.mode && L.mode.doc && L.mode.doc.numPages === 5);
  await __t.until('page 1 text', () => document.querySelector('.textLayer') && document.body.textContent.includes('Page 1'));
  await L.mode.command('rotate');
  await L.mode.save(true);
  await __t.until('the save', () => seen.includes('saved'), 60000);
  return { pages: L.mode.doc.numPages, seen };
});
