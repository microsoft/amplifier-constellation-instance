(() => {
  'use strict';
  const root = new URL('./', location.href);
  const $ = id => document.getElementById(id);
  let info = null, target = null, checking = false;
  function choosePlatform(value) {
    if (!['windows', 'mac', 'ubuntu'].includes(value)) throw new Error('Unknown platform');
    $('platform').value = value;
    for (const os of ['windows', 'mac', 'ubuntu']) $(os).hidden = os !== value;
  }
  async function check() {
    if (!info || checking) return;
    checking = true; $('check').disabled = true;
    $('check-result').textContent = 'Checking the secure connection…';
    try {
      const response = await fetch(new URL('welcome/check', target + '/'), {credentials:'omit', cache:'no-store', redirect:'error', signal:AbortSignal.timeout(6000)});
      const result = await response.json();
      if (!response.ok || result.instance_id !== info.instance_id) throw new Error('Unexpected installation');
      $('check-result').textContent = 'This browser can open the secure page. You can continue.';
    } catch {
      $('check-result').textContent = 'We couldn’t confirm the connection. If the secure page shows a certificate warning, follow the steps below. A network or browser setting can also prevent this check.';
    } finally {checking = false; $('check').disabled = false;}
  }
  function openSecure() { if (target) location.assign(target); }
  window.agentInstanceWelcome = Object.freeze({
    getState: () => ({platform:$('platform').value,checking,result:$('check-result').textContent,target,info}),
    selectPlatform:choosePlatform, check, openSecure,
    downloadCertificate: () => { $('download').click(); },
    showDetails: (index,open=true) => { const detail = document.querySelectorAll('details')[index]; if(!detail) throw new Error('Unknown detail'); detail.open = Boolean(open); }
  });
  $('platform').addEventListener('change', e => choosePlatform(e.target.value));
  $('check').addEventListener('click', check);
  (async () => {
    try {
      const response = await fetch(new URL('welcome/info',root), {credentials:'omit',cache:'no-store'});
      if (!response.ok) throw new Error('Connection details are unavailable');
      info = await response.json();
      const urls = (info.https_urls || []).filter(u => {try {return new URL(u).protocol === 'https:';} catch {return false;}});
      target = urls.find(u => new URL(u).hostname === location.hostname && new URL(u).port === String(info.https_port)) || urls.find(u => new URL(u).hostname === location.hostname) || info.preferred_url || urls[0];
      if (!target || new URL(target).protocol !== 'https:') throw new Error('Secure address is not ready');
      target = target.replace(/\/$/,'');
      for(const id of ['open','skip']) $(id).href = target;
      $('local-address').textContent = target;
      $('certificate-guide').hidden = !info.download_available;
      $('custom').hidden = info.download_available;
      $('certificate-name').textContent = info.certificate_name;
      document.querySelectorAll('.cert-name').forEach(el => el.textContent = info.certificate_name);
      $('fingerprint').textContent = (info.certificate_sha256 || '').match(/.{1,2}/g)?.join(':') || 'Not available';
      const filename = `agent-instance-${info.instance_id.slice(0,8)}.crt`;
      $('download').href = new URL('welcome/root.crt',root);
      $('download').download = filename;
      $('ubuntu-command').textContent = `sudo cp ~/Downloads/${filename} /usr/local/share/ca-certificates/\nsudo update-ca-certificates`;
      if (info.preferred_url && new URL(info.preferred_url).hostname.endsWith('.ts.net')) {
        $('preferred').hidden = false; $('preferred-link').href = info.preferred_url;
        $('preferred-address').textContent = info.preferred_url;
      }
      const platform = /Mac/i.test(navigator.platform) ? 'mac' : /Linux/i.test(navigator.platform) ? 'ubuntu' : 'windows';
      choosePlatform(platform); $('loading').hidden = true; $('ready').hidden = false;
      await check();
    } catch (error) { $('loading').textContent = `Could not load connection help. ${error.message}. Refresh this page or use the HTTPS address printed by the installer.`; }
  })();
})();
