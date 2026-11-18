// Leaflet opened with sample.docx: shown, edited and saved, then exported as a PDF.
__t.run('leaflet-doc', async () => {
  const L = await __t.until('the page', () => window.Leaflet);
  const seen = [];
  const handle = L.handle.bind(L);
  L.handle = (m) => { seen.push(m.what + (m.export ? ':export' : '')); return handle(m); };
  await __t.until('the document', () => document.querySelector('#doc') && document.querySelector('#doc').textContent.includes('Quarterly notes'));
  const p = document.createElement('p');
  p.textContent = 'Edited by the test.';
  document.querySelector('#doc').append(p);
  await L.mode.save(false);
  await __t.until('the save', () => seen.includes('saved'));
  L.mode.command('export-pdf');
  await __t.until('the export', () => seen.includes('saved:export'), 90000);
  return { seen };
});
