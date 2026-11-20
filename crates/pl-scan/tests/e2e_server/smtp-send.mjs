// Local SMTP client: node smtp-send.mjs <port> <dev.json> <file.pdf> <filename>
import net from 'node:net'
import fs from 'node:fs'
const [, , port, devFile, pdfFile, name] = process.argv
const token = JSON.parse(fs.readFileSync(devFile, 'utf8')).token
const pdf = fs.readFileSync(pdfFile).toString('base64').replace(/.{76}/g, '$&\r\n')
const msg = [
  'From: hp@printer.local', 'To: paperrust@nas.local', 'Subject: Scan', 'MIME-Version: 1.0',
  'Content-Type: multipart/mixed; boundary="B0"', '', '--B0', 'Content-Type: text/plain', '', 'Scan from HP', '--B0',
  `Content-Type: application/pdf; name="${name}"`, `Content-Disposition: attachment; filename="${name}"`,
  'Content-Transfer-Encoding: base64', '', pdf, '--B0--', '.', ''].join('\r\n')
const steps = ['EHLO hp-officejet', `AUTH PLAIN ${Buffer.from(`\0hp\0${token}`).toString('base64')}`, 'MAIL FROM:<hp@printer.local>', 'RCPT TO:<paperrust@nas.local>', 'DATA', msg, 'QUIT']
const s = net.connect(Number(port), '127.0.0.1')
let buf = '', i = -1
s.on('data', (d) => {
  buf += d.toString()
  const lines = buf.split('\r\n'); buf = lines.pop()
  for (const l of lines) {
    console.log('S:', l.replace(token, '<token>'))
    if (l.length >= 4 && l[3] === ' ') {
      i++
      if (i < steps.length) { const c = steps[i]; console.log('C:', c.startsWith('AUTH') ? 'AUTH PLAIN <hidden>' : c.length > 80 ? `<message ${c.length} bytes>` : c); s.write(c.endsWith('\r\n') ? c : c + '\r\n') }
    }
  }
})
s.on('close', () => console.log('closed'))
