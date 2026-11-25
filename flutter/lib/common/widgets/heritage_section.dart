// Heritage: Computers or Phones, above the device lists (Recent, Favorites, Address book, Accessible devices,
// and Discovered under Computers only). A device is a phone when it reports the iOS platform: drivers' iPhones
// served by the Heritage phone stand-in. The choice is remembered on this PC.
import 'package:get/get.dart';

import '../../consts.dart';
import '../../models/peer_model.dart';
import '../../models/platform_model.dart';

/// True while the Phones side is showing.
final RxBool heritagePhones = false.obs;

/// Reads the remembered side; called once when the device lists are first built.
void loadHeritageSection() {
  heritagePhones.value = bind.getLocalFlutterOption(k: kOptionHeritageSection) == 'phones';
}

void setHeritageSection({required bool phones}) {
  heritagePhones.value = phones;
  bind.setLocalFlutterOption(k: kOptionHeritageSection, v: phones ? 'phones' : 'computers');
}

/// Entries with no platform yet (never connected to) count as computers.
bool isHeritagePhone(Peer peer) => peer.platform.toLowerCase() == kPeerPlatformIOS.toLowerCase();

/// Whether a device belongs in the side now showing.
bool inHeritageSection(Peer peer) => isHeritagePhone(peer) == heritagePhones.value;
