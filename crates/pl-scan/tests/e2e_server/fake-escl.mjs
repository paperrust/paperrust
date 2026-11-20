// Standalone test scanner (eSCL) for the E2E run: node fake-escl.mjs <port> <caps.xml> <page.jpg> [sheets]
import http from 'node:http'
import fs from 'node:fs'
const [, , port, capsFile, jpgFile, sheetsArg] = process.argv
const caps = fs.readFileSync(capsFile)
const jpg = fs.readFileSync(jpgFile)
const sheets = Number(sheetsArg ?? 2)
let job = 0, delivered = 0, duplex = false, feeder = false
const log = (...a) => console.log(new Date().toISOString(), ...a)
http.createServer((req, res) => {
  log(req.method, req.url)
  if (req.url === '/eSCL/ScannerCapabilities') { res.writeHead(200, { 'content-type': 'text/xml' }); return res.end(caps) }
  if (req.url === '/eSCL/ScannerStatus') {
    res.writeHead(200, { 'content-type': 'text/xml' })
    return res.end('<?xml version="1.0"?><scan:ScannerStatus xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm"><pwg:State>Idle</pwg:State><scan:AdfState>ScannerAdfLoaded</scan:AdfState></scan:ScannerStatus>')
  }
  if (req.method === 'POST' && req.url === '/eSCL/ScanJobs') {
    let body = ''
    req.on('data', (c) => (body += c))
    req.on('end', () => {
      log('settings', body.replace(/\s+/g, ' '))
      duplex = body.includes('<scan:Duplex>true</scan:Duplex>')
      feeder = body.includes('<pwg:InputSource>Feeder</pwg:InputSource>')
      job++; delivered = 0
      res.writeHead(201, { location: `/eSCL/ScanJobs/job${job}` }); res.end()
    })
    return
  }
  const m = req.url.match(/^\/eSCL\/ScanJobs\/job\d+\/NextDocument$/)
  if (m) {
    const total = feeder ? (duplex ? sheets * 2 : sheets) : 1
    if (delivered >= total) { res.writeHead(404); return res.end() }
    delivered++
    res.writeHead(200, { 'content-type': 'image/jpeg' }); return res.end(jpg)
  }
  if (req.method === 'DELETE') { res.writeHead(200); return res.end() }
  res.writeHead(404); res.end()
}).listen(Number(port), '127.0.0.1', () => log('fake eSCL on', port))
