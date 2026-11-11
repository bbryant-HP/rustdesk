// Heritage Help Desk: the Device info panel.
//
// Shows what a device reports to the Heritage server: live CPU, memory and disk use, network
// adapters with their addresses, and hardware details. Opened from the toolbar during a
// session (a floating panel that leaves the remote screen usable) or from a device's
// right-click menu (a dialog). Needs the technician to be signed in.
//
// Kept in its own file so merging upstream RustDesk releases only touches a few lines.

import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;

import '../../common.dart';
import '../../models/platform_model.dart';

const _refreshEvery = Duration(seconds: 3);

Future<Map<String, dynamic>> fetchDeviceInfo(String id) async {
  final api = await bind.mainGetApiServer();
  if (api.isEmpty) throw 'No server is set up.';
  final token = bind.mainGetLocalOption(key: 'access_token');
  if (token.isEmpty) throw 'Sign in to see device info.';
  final resp = await http
      .get(Uri.parse('$api/api/heritage/device/${Uri.encodeComponent(id)}'),
          headers: getHttpHeaders())
      .timeout(const Duration(seconds: 10));
  Map<String, dynamic> body = {};
  try {
    body = jsonDecode(utf8.decode(resp.bodyBytes)) as Map<String, dynamic>;
  } catch (_) {}
  if (resp.statusCode != 200 || body.containsKey('error')) {
    throw body['error'] ?? 'The server answered ${resp.statusCode}.';
  }
  return body;
}

bool deviceInfoAvailable() =>
    bind.mainGetLocalOption(key: 'access_token').isNotEmpty;

/// The right-click menu's Device info: a dialog that keeps itself up to date.
void showDeviceInfoDialog(String id) {
  gFFI.dialogManager.show((setState, close, context) {
    return CustomAlertDialog(
      title: Row(children: [
        const Icon(Icons.monitor_heart_outlined, size: 20),
        const SizedBox(width: 8),
        Text('Device info'),
      ]),
      contentBoxConstraints:
          const BoxConstraints(minWidth: 420, maxWidth: 520, maxHeight: 560),
      content: SingleChildScrollView(child: DeviceInfoView(id: id)),
      actions: [dialogButton('Close', onPressed: close)],
      onCancel: close,
      onSubmit: close,
    );
  }, clickMaskDismiss: true, backDismiss: true);
}

final Map<String, OverlayEntry> _panels = {};

/// The session toolbar's Device info: a floating panel, shown or hidden with each press.
void toggleDeviceInfoPanel(BuildContext context, String id) {
  final open = _panels.remove(id);
  if (open != null) {
    open.remove();
    return;
  }
  final overlay = Overlay.maybeOf(context, rootOverlay: true);
  if (overlay == null) {
    showDeviceInfoDialog(id);
    return;
  }
  late OverlayEntry entry;
  entry = OverlayEntry(
      builder: (_) => _FloatingPanel(
            id: id,
            onClose: () {
              if (_panels[id] == entry) _panels.remove(id);
              entry.remove();
            },
          ));
  _panels[id] = entry;
  overlay.insert(entry);
}

class _FloatingPanel extends StatefulWidget {
  final String id;
  final VoidCallback onClose;
  const _FloatingPanel({required this.id, required this.onClose});

  @override
  State<_FloatingPanel> createState() => _FloatingPanelState();
}

class _FloatingPanelState extends State<_FloatingPanel> {
  Offset? _pos;
  static const _width = 360.0;

  @override
  Widget build(BuildContext context) {
    final size = MediaQuery.of(context).size;
    final pos = _pos ?? Offset(size.width - _width - 16, 56);
    final theme = Theme.of(context);
    return Positioned(
      left: pos.dx.clamp(0, (size.width - _width).clamp(0, double.infinity)),
      top: pos.dy.clamp(0, (size.height - 80).clamp(0, double.infinity)),
      width: _width,
      child: Material(
        elevation: 8,
        borderRadius: BorderRadius.circular(8),
        color: theme.dialogBackgroundColor,
        clipBehavior: Clip.antiAlias,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: size.height * 0.75),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            GestureDetector(
              onPanUpdate: (d) => setState(() => _pos = pos + d.delta),
              child: MouseRegion(
                cursor: SystemMouseCursors.move,
                child: Container(
                  color: theme.colorScheme.primary,
                  padding: const EdgeInsets.fromLTRB(12, 6, 4, 6),
                  child: Row(children: [
                    const Icon(Icons.monitor_heart_outlined,
                        size: 18, color: Colors.white),
                    const SizedBox(width: 8),
                    const Expanded(
                        child: Text('Device info',
                            style: TextStyle(
                                color: Colors.white,
                                fontWeight: FontWeight.w600))),
                    IconButton(
                      tooltip: 'Close',
                      iconSize: 18,
                      visualDensity: VisualDensity.compact,
                      icon: const Icon(Icons.close, color: Colors.white),
                      onPressed: widget.onClose,
                    ),
                  ]),
                ),
              ),
            ),
            Flexible(
              child: SingleChildScrollView(
                padding: const EdgeInsets.all(12),
                child: DeviceInfoView(id: widget.id),
              ),
            ),
          ]),
        ),
      ),
    );
  }
}

class DeviceInfoView extends StatefulWidget {
  final String id;
  const DeviceInfoView({Key? key, required this.id}) : super(key: key);

  @override
  State<DeviceInfoView> createState() => _DeviceInfoViewState();
}

class _DeviceInfoViewState extends State<DeviceInfoView> {
  Map<String, dynamic>? _info;
  String? _error;
  Timer? _timer;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _load();
    _timer = Timer.periodic(_refreshEvery, (_) => _load());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    if (_busy) return;
    _busy = true;
    try {
      final info = await fetchDeviceInfo(widget.id);
      if (mounted) {
        setState(() {
          _info = info;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      _busy = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final info = _info;
    if (info == null) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 24),
        child: Center(
            child: _error == null
                ? const CircularProgressIndicator()
                : Text(_error!, textAlign: TextAlign.center)),
      );
    }
    final stats = _map(info['stats']);
    final hw = _map(info['hw']);
    final net = (info['net'] is List) ? info['net'] as List : const [];
    final ip = _map(info['ip']);
    final children = <Widget>[
      _header(context, info, stats),
      if (_error != null)
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text('Couldn\'t refresh: $_error',
              style: TextStyle(color: Theme.of(context).colorScheme.error)),
        ),
      _section(context, 'Usage'),
      if (stats.isEmpty)
        _muted(context,
            'This device hasn\'t sent usage yet. It needs the Heritage-HelpDesk version with Device info.')
      else
        ..._usage(context, stats),
      _section(context, 'Network'),
      _row(context, 'Public IP', _str(ip['public']).isEmpty ? 'Unknown' : _str(ip['public']),
          note: _str(ip['public_note'])),
      if (_str(ip['seen']).isNotEmpty) _row(context, 'Seen by server', _str(ip['seen'])),
      for (final n in net) ..._adapter(context, _map(n)),
      if (net.isEmpty) _muted(context, 'No network details reported yet.'),
      _section(context, 'Computer'),
      _row(context, 'Name', _str(info['hostname'])),
      _row(context, 'Manufacturer', _str(hw['manufacturer'])),
      _row(context, 'Model', _str(hw['model'])),
      _row(context, 'Serial number', _str(hw['serial'])),
      _row(context, 'Motherboard', _str(hw['board'])),
      _row(context, 'BIOS', [_str(hw['bios']), _str(hw['bios_date'])].where((s) => s.isNotEmpty).join(', ')),
      _row(context, 'Processor', _str(hw['cpu']).isNotEmpty ? _str(hw['cpu']) : _str(info['cpu'])),
      if (hw['cores'] != null)
        _row(context, 'Cores', '${hw['cores']} cores, ${hw['threads'] ?? '?'} threads'),
      _row(context, 'Memory', hw['ram'] is num ? _bytes(hw['ram']) : _str(info['memory'])),
      for (final d in (hw['disks'] is List ? hw['disks'] as List : const []))
        _row(context, 'Drive', _physicalDisk(_map(d))),
      _section(context, 'Windows'),
      _row(context, 'Edition', _str(hw['os']).isNotEmpty ? _str(hw['os']) : _str(info['os'])),
      _row(context, 'Version', [_str(hw['os_display']), _str(hw['os_build']), _str(hw['os_arch'])].where((s) => s.isNotEmpty).join(', ')),
      _row(context, 'Signed-in user', _str(info['username'])),
      _row(context, hw['joined'] == true ? 'Domain' : 'Workgroup', _str(hw['domain'])),
      _row(context, 'Installed', _str(hw['installed'])),
      _row(context, 'Last boot', _str(hw['last_boot'])),
      _row(context, 'Help Desk version', _str(info['version'])),
      _row(context, 'ID', widget.id),
    ];
    return SelectionArea(
      child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisSize: MainAxisSize.min,
          children: children),
    );
  }

  Widget _header(BuildContext context, Map info, Map stats) {
    final online = info['online'] == true;
    final age = stats['age'];
    final when = age is num ? 'Updated ${_ago(age.toInt())}' : 'No live data';
    return Row(children: [
      Icon(Icons.circle, size: 10, color: online ? Colors.green : Colors.grey),
      const SizedBox(width: 6),
      Expanded(
        child: Text(_str(info['hostname']).isEmpty ? widget.id : _str(info['hostname']),
            style: const TextStyle(fontWeight: FontWeight.w600, fontSize: 15),
            overflow: TextOverflow.ellipsis),
      ),
      Text(online ? when : 'Offline', style: _mutedStyle(context)),
    ]);
  }

  List<Widget> _usage(BuildContext context, Map stats) {
    final memTotal = (stats['mem_total'] as num?) ?? 0;
    final memUsed = (stats['mem_used'] as num?) ?? 0;
    final cpu = (stats['cpu'] as num?) ?? 0;
    final disks = stats['disks'] is List ? stats['disks'] as List : const [];
    return [
      _meter(context, 'CPU', cpu / 100, '${cpu.toStringAsFixed(0)}%'),
      _meter(context, 'Memory', memTotal > 0 ? memUsed / memTotal : 0,
          '${_bytes(memUsed)} of ${_bytes(memTotal)}'),
      for (final d in disks)
        if (_map(d)['removable'] != true)
          _meter(
              context,
              'Drive ${_str(_map(d)['mount'])}',
              _diskUsed(_map(d)),
              '${_bytes(_map(d)['free'] ?? 0)} free of ${_bytes(_map(d)['total'] ?? 0)}'),
      if (stats['uptime'] is num) _row(context, 'Up for', _duration((stats['uptime'] as num).toInt())),
    ];
  }

  List<Widget> _adapter(BuildContext context, Map n) {
    final ipv4 = (n['ipv4'] is List) ? (n['ipv4'] as List).join(', ') : '';
    final dns = (n['dns'] is List) ? (n['dns'] as List).join(', ') : '';
    final type = _str(n['type']) == 'Wireless80211' ? 'Wi-Fi' : _str(n['type']) == 'Ethernet' ? 'Wired' : _str(n['type']);
    final speed = n['speed_mbps'] is num && n['speed_mbps'] > 0
        ? (n['speed_mbps'] >= 1000 ? '${(n['speed_mbps'] / 1000).toStringAsFixed(n['speed_mbps'] % 1000 == 0 ? 0 : 1)} Gbps' : '${n['speed_mbps']} Mbps')
        : '';
    return [
      Padding(
        padding: const EdgeInsets.only(top: 6, bottom: 2),
        child: Text([_str(n['name']), type, speed].where((s) => s.isNotEmpty).join(' · '),
            style: const TextStyle(fontWeight: FontWeight.w600)),
      ),
      if (_str(n['description']).isNotEmpty) _muted(context, _str(n['description'])),
      _row(context, 'IP address', ipv4),
      _row(context, 'MAC address', _str(n['mac'])),
      _row(context, 'Gateway', _str(n['gateway'])),
      _row(context, 'DNS', dns),
    ];
  }

  Widget _meter(BuildContext context, String label, num fraction, String text) {
    final f = fraction.clamp(0, 1).toDouble();
    final color = f >= 0.9 ? Colors.red : f >= 0.75 ? Colors.orange : Theme.of(context).colorScheme.primary;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Row(children: [
          Expanded(child: Text(label)),
          Text(text, style: _mutedStyle(context)),
        ]),
        const SizedBox(height: 3),
        ClipRRect(
          borderRadius: BorderRadius.circular(3),
          child: LinearProgressIndicator(
            value: f,
            minHeight: 6,
            color: color,
            backgroundColor: color.withOpacity(0.15),
          ),
        ),
      ]),
    );
  }

  Widget _section(BuildContext context, String title) => Padding(
        padding: const EdgeInsets.only(top: 14, bottom: 4),
        child: Text(title.toUpperCase(),
            style: TextStyle(
                fontSize: 11,
                letterSpacing: 0.8,
                fontWeight: FontWeight.w700,
                color: Theme.of(context).colorScheme.primary)),
      );

  Widget _row(BuildContext context, String label, String value, {String note = ''}) {
    if (value.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        SizedBox(width: 120, child: Text(label, style: _mutedStyle(context))),
        Expanded(child: Text(note.isEmpty ? value : '$value ($note)')),
      ]),
    );
  }

  Widget _muted(BuildContext context, String text) => Padding(
        padding: const EdgeInsets.symmetric(vertical: 2),
        child: Text(text, style: _mutedStyle(context)),
      );

  TextStyle _mutedStyle(BuildContext context) =>
      TextStyle(color: Theme.of(context).hintColor, fontSize: 13);
}

Map _map(dynamic v) => v is Map ? v : const {};
String _str(dynamic v) => v == null ? '' : v.toString().trim();

double _diskUsed(Map d) {
  final total = (d['total'] as num?) ?? 0;
  final free = (d['free'] as num?) ?? 0;
  return total > 0 ? (total - free) / total : 0;
}

String _physicalDisk(Map d) {
  final media = _str(d['media']);
  final kind = media == 'Unspecified' || media.isEmpty ? '' : media;
  final parts = [
    _str(d['model']),
    if (d['size'] is num) _bytes(d['size']),
    [kind, _str(d['bus'])].where((s) => s.isNotEmpty).join(' '),
    if (_str(d['health']).isNotEmpty && _str(d['health']) != 'Healthy') 'Health: ${_str(d['health'])}',
  ];
  return parts.where((s) => s.isNotEmpty).join(', ');
}

String _bytes(dynamic n) {
  final v = (n is num) ? n.toDouble() : 0.0;
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var i = 0;
  var x = v;
  while (x >= 1024 && i < units.length - 1) {
    x /= 1024;
    i++;
  }
  return '${x.toStringAsFixed(x >= 100 || i == 0 ? 0 : 1)} ${units[i]}';
}

String _duration(int s) {
  final d = s ~/ 86400, h = (s % 86400) ~/ 3600, m = (s % 3600) ~/ 60;
  if (d > 0) return '$d day${d == 1 ? '' : 's'}, $h hr';
  if (h > 0) return '$h hr $m min';
  return '$m min';
}

String _ago(int s) {
  if (s < 5) return 'just now';
  if (s < 60) return '${s}s ago';
  if (s < 3600) return '${s ~/ 60} min ago';
  return '${s ~/ 3600} hr ago';
}
