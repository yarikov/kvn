# What kvn protects

kvn hides your traffic from the local network and your internet provider by
sending it through a VPN server. This page explains which traffic goes through
the tunnel, which deliberately does not, and what the network can still see.

- [Routing modes](#routing-modes)
- [DNS](#dns)
- [Kill switch](#kill-switch)
- [IPv4 only](#ipv4-only)
- [What the network still sees](#what-the-network-still-sees)
- [Requests kvn makes itself](#requests-kvn-makes-itself)
- [Known limitations](#known-limitations)
- [Checking it yourself](#checking-it-yourself)

## Routing modes

The routing mode decides which traffic uses the tunnel. Change it in
**Settings › Routing** (`Space r`): pick a country **Region** first, and the
**Mode** row then offers Bypass and Only next to Global.

| Mode | Through the VPN | Directly, past the VPN |
|------|-----------------|------------------------|
| Global | All IPv4 traffic | Only the local network your computer is directly connected to |
| Bypass (`bypass_ru`, …) | All IPv4 traffic except the region's sites | The region's sites and IP addresses, and your local network |
| Only (`only_ru`, …) | Only the region's sites | Everything else, including your local network |

A connection belongs to the region when its site name is on the region's list
or its address is in the region's address list. kvn reads the name from the
connection itself, so this works with fake-IP on or off; a site matched by
neither uses the tunnel in Bypass and goes directly in Only. To compare an
address with the list, kvn may look the name up again through your main DNS
server, along the same path as your other DNS questions.

The region's lists are files kvn downloads. Until they are on disk, a
connection still starts but without the region's rules: the local network
still goes directly in Bypass and Only, and service overrides whose files are
present still apply, but everything else follows the mode's default — the
tunnel in Bypass, and directly in Only, including the region's own sites.

The local network your computer is directly connected to — for example
`192.168.1.0/24` with your router — is reached directly in every mode: the
system routes it before traffic reaches the tunnel. Other private addresses,
such as another network behind your router or a work network, are sent through
the VPN in Global mode, where they are usually unreachable; Bypass and Only
send every private address directly.

The tunnel carries IPv4 only; IPv6 is blocked in every mode, see
[IPv4 only](#ipv4-only).

Direct traffic leaves through your normal connection, so your provider sees
where it goes, exactly as it would without a VPN. Bypass and Only exist for
that purpose: Bypass keeps domestic sites fast and reachable, and Only reaches
services that refuse foreign IP addresses while everything else stays direct.

The Steam and Telegram rows of the same screen override the mode for those
services. A service set to `direct` always skips the tunnel; one set to
`proxy` always uses it. An override needs its own rule-set files, which kvn
downloads through the tunnel. Until they are downloaded and kvn has
reconnected, the service follows the routing mode. Turning an override on
while connected does both automatically; files that were missing when a
connection started apply only from the next connection.

## DNS

Before opening a site, your computer asks a DNS server for its address. These
questions name every site you visit, so where they go matters as much as the
traffic itself.

kvn answers DNS questions itself and forwards them to the servers of the
selected DNS preset (see the [configuration guide](configuration.md#dns)):

- **Global and Bypass:** questions to public DNS servers travel through the
  tunnel. Your provider does not see them.
- **Only:** every DNS question is sent directly, like the rest of the traffic
  outside the region.
- **Local DNS servers** — a router such as `192.168.1.1`, a resolver on
  `127.0.0.1`, or a link-local or `100.64.0.0/10` address — are always queried
  directly, because they are only reachable on your own network.
- **The System local preset** asks your system's resolver, which is always
  queried directly, in every mode. Where it forwards the questions — often your
  router or your provider — is outside kvn's control, so use another preset if
  your provider must not see them.
- **The VPN server's own name** is looked up directly, through a copy of your
  main DNS server, because the tunnel does not exist yet when it is needed.
  The same holds for its Encrypted Client Hello key: a profile whose ECH
  config comes from DNS asks that copy for the TLS server name's `HTTPS`
  record, which carries the key. If the active DNS preset has a rule with an
  IP rule-set (such as `geoip-ru`), sing-box cannot limit the rule to that
  record, so every lookup of that one name — including your programs' — goes
  to the copy. That query names the server ECH hides, so over
  plain DNS (`local`, `udp`, `tcp`) anyone on the path sees it; kvn shows a
  warning after connecting in that case. A DoH, DoT or DoQ preset, or a link
  that carries the ECH config itself, avoids it.

Fake-IP adds one more direct path. It is off by default; when it is on, or
when a DNS rule sends some names to it, programs receive a placeholder address
for those names, and kvn looks up the real one only when it opens the
connection. Which way that lookup goes depends on the rule that
matched:

- **A site sent directly because of its name** — on the region's list in
  Bypass, or on the list of a service set to `direct` in any mode — is looked
  up through the direct copy of your main DNS server, past the tunnel. Your
  DNS provider then sees these names together with your real IP address. The
  site itself sees that address anyway, and your provider sees the connection
  either way.
- **A site sent directly because of its address** is looked up through the
  tunnel first.
- **While a Steam or Telegram override is on**, kvn looks every other name up
  through the tunnel before it checks the region's list, so region sites no
  longer take the first path; the service's own names still do when it is set
  to `direct`.

Without fake-IP, programs get real addresses from kvn, through the tunnel in
Global and Bypass, and the direct connection needs no lookup of its own.

Encrypted DNS (`https`, `tls`, `quic`) hides the questions even when they are
sent directly; plain DNS (`udp`, `tcp`) shows them to anyone on the path. The
built-in presets other than System local use encrypted DNS.

HTTP, SSH, ShadowTLS, NaiveProxy and SOCKS 4/4a profiles cannot carry UDP
through the tunnel. In Global and Bypass modes kvn therefore sends a public `udp` or
`quic` DNS server's questions directly, past the VPN, and shows a warning after
connecting. Use an `https`, `tls` or `tcp` server with these profiles to keep
DNS in the tunnel.

UDP from other programs is refused on these profiles, not sent around the
tunnel: the program gets an immediate error. Browsers then fall back from
HTTP/3 to TCP, so websites keep working, but UDP-only traffic such as voice
and video calls or games does not work. VLESS, VMess, Trojan, Shadowsocks,
Hysteria 2, TUIC, AnyTLS and SOCKS5 profiles carry UDP.

## Kill switch

The [kill switch](../README.md#kill-switch-setup-optional) is a firewall rule
set that drops traffic leaving outside the tunnel. It protects you when kvn is
not carrying your traffic: before the tunnel is up, after it drops, or when the
daemon has stopped. Other programs then cannot reach the internet directly.

It does not override the routing decisions above. Traffic that kvn itself sends
directly — Bypass and Only destinations, `direct` service routes, the DNS
traffic described above, and the connection to the VPN server — is marked by
kvn and allowed through, because sending it directly is the purpose of those
settings.

Exactly what the kill switch lets through, besides the tunnel interface
(`kvn*`) and loopback:

- packets kvn marks (firewall mark `0x29a`), as described above;
- incoming replies on established and related connections, so the exceptions
  in this list work in both directions; a direct connection opened before the
  kill switch came on stops sending as soon as it does;
- the local network: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
  `fc00::/7` and `fe80::/10`, except `fc00::/18`, kvn's default IPv6 fake-IP
  range, so fake addresses programs still hold do not leave outside the tunnel
  (a custom `inet6_range` inside the local ranges is not covered);
- all ICMP and ICMPv6 (ping, path MTU discovery, IPv6 neighbor discovery);
- DHCP and DHCPv6, so the network connection itself keeps working.

Everything else leaving the computer is dropped — for example traffic to a
`169.254.0.0/16` link-local address, multicast and broadcast traffic such as
mDNS and SSDP, or a `100.64.0.0/10` address used by Tailscale. Traffic the
computer forwards for containers and virtual machines may leave only through
the tunnel, including connections they opened before the kill switch came on.

Latency tests (`t` / `T`) start a separate sing-box whose packets carry the
same mark, so they reach each tested VPN server directly even with the kill
switch on.

## IPv4 only

The tunnel carries IPv4 only. While kvn is connected:

- **IPv6 is blocked**, not sent around the tunnel. Programs that try an IPv6
  address get an immediate error and fall back to IPv4, so sites that offer
  both keep working; sites reachable only over IPv6 do not open.
- **The VPN server must be reachable over IPv4.** A profile whose address is
  an IPv6 address, or a name with only an IPv6 address, cannot connect.
- **DNS servers that kvn queries directly must be reachable over IPv4**: in
  Only mode, local servers, and the copy of the main server used to look up
  the VPN server's name. In Global and Bypass modes a public DNS server is
  queried through the tunnel, so an IPv6 address works there if the VPN
  server itself has IPv6.
- **Direct traffic is IPv4 too**: Bypass and Only destinations and `direct`
  service routes cannot be reached over IPv6.

Settings › DNS therefore offers only the IPv4 strategies. `Prefer IPv4` (the
default) still hands programs IPv6 addresses they cannot use, so they try them
first and fall back; `IPv4 only` stops that and avoids the failed attempts.

## What the network still sees

Even in Global mode, your provider can see:

- that you are connected to a VPN server, its address, and how much data
  flows;
- the lookup of the VPN server's name and the connection to your main DNS
  server used for it, if the server is configured by name;
- everything listed as direct for your routing mode and service routes.

The VPN server's operator sees the traffic that leaves the tunnel, as any VPN
provider does.

## Requests kvn makes itself

kvn's own downloads are ordinary traffic: while connected they go through the
tunnel (or directly, as your routing mode decides), and while disconnected
they go directly. With the kill switch on, kvn makes them only while the tunnel
is up.

- **Rule-sets** for the selected region and for Steam and Telegram overrides
  are downloaded from `raw.githubusercontent.com` (SagerNet and MetaCubeX
  repositories), on first use and on the `geo_routing.auto_update` schedule.
  The first download of a service's rule-sets waits for the tunnel; scheduled
  refreshes run like the others.
- **Subscriptions** are fetched from their URL with
  `User-Agent: kvn-tui/<version>`. A subscription with `send_hwid` also sends
  an installation identifier, the kernel version, and the system locale; see
  [Subscriptions](configuration.md#subscriptions).
- **Latency tests** request `settings.connectivity_probe.url`
  (`https://connectivitycheck.gstatic.com/generate_204` by default) through
  each tested VPN server, only when you press `t` or `T`.

## Known limitations

- Plain `udp` DNS servers are not encrypted. In Only mode, and for local
  servers, their questions are visible on the network they travel through.
- With fake-IP on, a DNS rule that uses an IP rule-set (`geoip-*`) prevents
  connecting until a rule sends queries to the fake-IP server; see the
  [configuration guide](configuration.md#dns).
- Settings › DNS applies to programs that use the system resolver. A browser
  with secure DNS enabled sends its own encrypted DNS requests: kvn routes them
  like any other traffic, but your DNS servers, rules and fake-IP do not apply
  to them.

## Checking it yourself

Plain `curl https://site` is not proof either way: a certificate error can
follow a connection that did leave the machine, and fake-IP hides which
address was used. Pin a real address and keep the real name instead.

First, with kvn disconnected **and the kill switch off** (it stays active
after a disconnect and would block this request), look up a real IPv6 address
of `ifconfig.co` and request it directly. If this control request fails, IPv6
connectivity without the VPN is not confirmed, and the check below proves
nothing. Looking the address up while kvn is connected may return a fake-IP
address or nothing at all.

```bash
dig +short AAAA ifconfig.co                               # kvn disconnected: pick <address>
curl -6 --resolve 'ifconfig.co:443:[<address>]' https://ifconfig.co   # should succeed
```

Then turn the kill switch back on if you use it, connect kvn, and repeat the
same request, watching your physical network interface `<interface>`:

```bash
sudo tcpdump -n -i <interface> host <address>            # in a second terminal
curl -6 --resolve 'ifconfig.co:443:[<address>]' https://ifconfig.co
```

While kvn is connected the request must fail and `tcpdump` must show no
packets to that address. With the kill switch on, the same must hold while kvn
reconnects (`r`) and after the `sing-box` process stops. Without the kill
switch, the system's normal network takes over as soon as sing-box stops, so
the request may then succeed: that is the gap the kill switch closes. The
same command with `-4` and a real IPv4 address of `ifconfig.co`, looked up
the same way while kvn and the kill switch are off (`dig +short A
ifconfig.co`), shows the VPN server's address while the tunnel works.
