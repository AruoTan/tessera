use std::{collections::BTreeMap, io, ptr};
use tessera_core::network::{Adapter, AdapterKind, Counters, physical_adapters};
use windows_sys::Win32::NetworkManagement::{
    IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, MIB_IF_ROW2,
        MIB_IF_TABLE2,
    },
    Ndis::{
        NdisPhysicalMedium802_3 as MEDIUM_ETHERNET,
        NdisPhysicalMediumNative802_11 as MEDIUM_NATIVE_WIFI,
        NdisPhysicalMediumUnspecified as MEDIUM_UNSPECIFIED,
        NdisPhysicalMediumWirelessLan as MEDIUM_WIFI, TUNNEL_TYPE_NONE,
    },
};

// MIB_IF_ROW2.InterfaceAndOperStatusFlags bitfield (netioapi.h).
const HARDWARE_INTERFACE: u8 = 1;
const FILTER_INTERFACE: u8 = 1 << 1;
const ENDPOINT_INTERFACE: u8 = 1 << 7;

struct Table(*mut MIB_IF_TABLE2);

impl Drop for Table {
    fn drop(&mut self) {
        // GetIfTable2 owns this allocation, including the variable-length row array.
        unsafe { FreeMibTable(self.0.cast()) };
    }
}

pub fn read() -> io::Result<BTreeMap<u64, Adapter>> {
    let mut table = ptr::null_mut();
    let status = unsafe { GetIfTable2(&mut table) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    if table.is_null() {
        return Err(io::Error::other("Windows returned no interface table"));
    }
    let table = Table(table);
    // Use the generated field offset and row stride: the API table includes padding.
    let rows = unsafe {
        std::slice::from_raw_parts(
            ptr::addr_of!((*table.0).Table).cast::<MIB_IF_ROW2>(),
            (*table.0).NumEntries as usize,
        )
    };
    Ok(physical_adapters(rows.iter().map(|row| {
        // LUID identifies an interface independently of its alias and MAC address.
        (unsafe { row.InterfaceLuid.Value }, adapter(row))
    })))
}

fn adapter(row: &MIB_IF_ROW2) -> Adapter {
    let flags = row.InterfaceAndOperStatusFlags._bitfield;
    let physical = flags & HARDWARE_INTERFACE != 0
        && flags & (FILTER_INTERFACE | ENDPOINT_INTERFACE) == 0
        && row.TunnelType == TUNNEL_TYPE_NONE;
    // Some legacy Wi-Fi drivers expose Ethernet interface type. Consult the
    // physical medium as well, and reject Bluetooth/WWAN despite Ethernet emulation.
    let kind = match (row.Type, row.PhysicalMediumType) {
        (IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211, MEDIUM_WIFI | MEDIUM_NATIVE_WIFI)
        | (IF_TYPE_IEEE80211, MEDIUM_UNSPECIFIED) => AdapterKind::Wifi,
        (IF_TYPE_ETHERNET_CSMACD, MEDIUM_ETHERNET | MEDIUM_UNSPECIFIED) => AdapterKind::Ethernet,
        _ => AdapterKind::Other,
    };
    let end = row
        .Alias
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(row.Alias.len());
    Adapter {
        name: String::from_utf16_lossy(&row.Alias[..end]),
        kind,
        physical,
        // These already include unicast, multicast and broadcast. Do not add
        // the individual category counters, or any virtual interface counters.
        counters: Counters {
            received: row.InOctets,
            transmitted: row.OutOctets,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::NetworkManagement::{
        IpHelper::MIB_IF_ROW2_0,
        Ndis::{NdisPhysicalMediumBluetooth, NdisPhysicalMediumWirelessWan},
    };

    #[test]
    fn windows_metadata_filters_virtual_filter_endpoint_and_tunnel_rows() {
        let mut row = MIB_IF_ROW2 {
            Type: IF_TYPE_ETHERNET_CSMACD,
            PhysicalMediumType: MEDIUM_UNSPECIFIED,
            InterfaceAndOperStatusFlags: MIB_IF_ROW2_0 {
                _bitfield: HARDWARE_INTERFACE,
            },
            InOctets: 1_000,
            InUcastOctets: 800,
            InMulticastOctets: 150,
            InBroadcastOctets: 50,
            OutOctets: 400,
            ..Default::default()
        };
        let eligible = |row: &MIB_IF_ROW2| !physical_adapters([(1, adapter(row))]).is_empty();
        assert!(eligible(&row));
        assert_eq!(adapter(&row).counters.received, 1_000);
        assert_eq!(adapter(&row).counters.transmitted, 400);
        for flags in [
            0,
            HARDWARE_INTERFACE | FILTER_INTERFACE,
            HARDWARE_INTERFACE | ENDPOINT_INTERFACE,
        ] {
            row.InterfaceAndOperStatusFlags._bitfield = flags;
            assert!(!eligible(&row));
        }
        row.InterfaceAndOperStatusFlags._bitfield = HARDWARE_INTERFACE;
        row.TunnelType = 1;
        assert!(!eligible(&row));
    }

    #[test]
    fn wifi_is_supported_and_ethernet_emulation_does_not_admit_bluetooth_or_wwan() {
        let mut row = MIB_IF_ROW2 {
            Type: IF_TYPE_IEEE80211,
            PhysicalMediumType: MEDIUM_NATIVE_WIFI,
            InterfaceAndOperStatusFlags: MIB_IF_ROW2_0 {
                _bitfield: HARDWARE_INTERFACE,
            },
            ..Default::default()
        };
        assert_eq!(adapter(&row).kind, AdapterKind::Wifi);
        row.Type = IF_TYPE_ETHERNET_CSMACD;
        row.PhysicalMediumType = MEDIUM_WIFI;
        assert_eq!(adapter(&row).kind, AdapterKind::Wifi);
        for medium in [NdisPhysicalMediumBluetooth, NdisPhysicalMediumWirelessWan] {
            row.PhysicalMediumType = medium;
            assert_eq!(adapter(&row).kind, AdapterKind::Other);
        }
    }
}
