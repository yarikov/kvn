# What kvn protects

kvn hides your traffic from the local network and your internet provider by
sending it through a VPN server. This page explains which traffic goes through
the tunnel, which deliberately does not, and what the network can still see.

- [Routing modes](#routing-modes)
- [DNS](#dns)
- [Kill switch](#kill-switch)
- [IPv4 only](#ipv4-only)
- [What the network still sees](#what-the-network-still-sees)
- [Known limitations](#known-limitations)
- [Checking it yourself](#checking-it-yourself)

## Routing modes

The routing mode decides which traffic uses the tunnel. Change it in
**Settings › Routing** (`Space r`): pick a country **Region** first, and the
**Mode** row then offers Bypass and Only next to Global.

| Mode | Through the VPN | Directly, past the VPN |
|------|-----------------|------------------------|
| Global | All IPv4 traffic | Nothing |
| Bypass (`bypass_ru`, …) | All IPv4 traffic except the region's sites | The region's sites and IP addresses, and your local network |
| Only (`only_ru`, …) | Only the region's sites | Everything else, including your local network |

A connection belongs to the region when its site name is on the region's list
or its address is in the region's address list. kvn reads the name from the
connection itself, so this works with fake-IP on or off; a site matched by
neither uses the tunnel in Bypass and goes directly in Only. To compare an
address with the list, kvn may look the name up again through your main DNS
server, along the same path as your other DNS questions.

The tunnel carries IPv4 only; IPv6 is blocked in every mode, see
[IPv4 only](#ipv4-only).

Direct traffic leaves through your normal connection, so your provider sees
where it goes, exactly as it would without a VPN. Bypass and Only exist for
that purpose: Bypass keeps domestic sites fast and reachable, and Only reaches
services that refuse foreign IP addresses while everything else stays direct.

The Steam and Telegram rows of the same screen override the mode for those
services. A service set to `direct` always skips the tunnel; one set to
`proxy` always uses it.

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
- **Local DNS servers** — a router such as `192.168.1.1` or a resolver on
  `127.0.0.1` — are always queried directly, because they are only reachable
  on your own network.
- **The VPN server's own name** is looked up directly, through a copy of your
  main DNS server, because the tunnel does not exist yet when it is needed.

Encrypted DNS (`https`, `tls`, `quic`) hides the questions even when they are
sent directly; plain DNS (`udp`, `tcp`) shows them to anyone on the path. The
built-in presets other than System local use encrypted DNS.

HTTP, SSH, ShadowTLS and SOCKS 4/4a profiles cannot carry UDP through the
tunnel. In Global and Bypass modes kvn therefore refuses to connect with them
while a public `udp` or `quic` DNS server is configured, rather than sending
those questions past the VPN. Use an `https`, `tls` or `tcp` server with these
profiles.

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
- all traffic on established and related connections, in both directions:
  turning the kill switch on does not cut direct connections that are already
  open;
- the local network: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
  `fc00::/7` and `fe80::/10`;
- all ICMP and ICMPv6 (ping, path MTU discovery, IPv6 neighbor discovery);
- DHCP and DHCPv6, so the network connection itself keeps working;
- the VPN server's address and port, opened for the handshake when connecting
  and closed on disconnect.

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
