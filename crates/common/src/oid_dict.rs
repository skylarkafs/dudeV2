/// Standard MIB and vendor base OID dictionary for human-readable SNMP walks and trap logs.
static STANDARD_OIDS: &[(&str, &str)] = &[
    // SNMPv2-MIB::system
    ("1.3.6.1.2.1.1.1", "SNMPv2-MIB::sysDescr"),
    ("1.3.6.1.2.1.1.2", "SNMPv2-MIB::sysObjectID"),
    ("1.3.6.1.2.1.1.3", "SNMPv2-MIB::sysUpTime"),
    ("1.3.6.1.2.1.1.4", "SNMPv2-MIB::sysContact"),
    ("1.3.6.1.2.1.1.5", "SNMPv2-MIB::sysName"),
    ("1.3.6.1.2.1.1.6", "SNMPv2-MIB::sysLocation"),
    ("1.3.6.1.2.1.1.7", "SNMPv2-MIB::sysServices"),
    // IF-MIB
    ("1.3.6.1.2.1.2.2.1.1", "IF-MIB::ifIndex"),
    ("1.3.6.1.2.1.2.2.1.2", "IF-MIB::ifDescr"),
    ("1.3.6.1.2.1.2.2.1.3", "IF-MIB::ifType"),
    ("1.3.6.1.2.1.2.2.1.5", "IF-MIB::ifSpeed"),
    ("1.3.6.1.2.1.2.2.1.6", "IF-MIB::ifPhysAddress"),
    ("1.3.6.1.2.1.2.2.1.7", "IF-MIB::ifAdminStatus"),
    ("1.3.6.1.2.1.2.2.1.8", "IF-MIB::ifOperStatus"),
    ("1.3.6.1.2.1.2.2.1.10", "IF-MIB::ifInOctets"),
    ("1.3.6.1.2.1.2.2.1.14", "IF-MIB::ifInErrors"),
    ("1.3.6.1.2.1.2.2.1.16", "IF-MIB::ifOutOctets"),
    ("1.3.6.1.2.1.2.2.1.20", "IF-MIB::ifOutErrors"),
    ("1.3.6.1.2.1.31.1.1.1.1", "IF-MIB::ifName"),
    ("1.3.6.1.2.1.31.1.1.1.6", "IF-MIB::ifHCInOctets"),
    ("1.3.6.1.2.1.31.1.1.1.10", "IF-MIB::ifHCOutOctets"),
    ("1.3.6.1.2.1.31.1.1.1.15", "IF-MIB::ifHighSpeed"),
    ("1.3.6.1.2.1.31.1.1.1.18", "IF-MIB::ifAlias"),
    // SNMPv2-MIB Traps & SNMPv2-SMI
    ("1.3.6.1.6.3.1.1.4.1", "SNMPv2-MIB::snmpTrapOID"),
    ("1.3.6.1.6.3.1.1.5.1", "SNMPv2-MIB::coldStart"),
    ("1.3.6.1.6.3.1.1.5.2", "SNMPv2-MIB::warmStart"),
    ("1.3.6.1.6.3.1.1.5.3", "IF-MIB::linkDown"),
    ("1.3.6.1.6.3.1.1.5.4", "IF-MIB::linkUp"),
    ("1.3.6.1.6.3.1.1.5.5", "SNMPv2-MIB::authenticationFailure"),
    ("1.3.6.1.6.3.15.1.1.2", "SNMP-USER-BASED-SM-MIB::usmStatsNotInTimeWindows"),
    ("1.3.6.1.6.3.15.1.1.4", "SNMP-USER-BASED-SM-MIB::usmStatsUnknownEngineIDs"),
    // HOST-RESOURCES-MIB
    ("1.3.6.1.2.1.25.1.1", "HOST-RESOURCES-MIB::hrSystemUptime"),
    ("1.3.6.1.2.1.25.2.3.1.3", "HOST-RESOURCES-MIB::hrStorageDescr"),
    ("1.3.6.1.2.1.25.2.3.1.4", "HOST-RESOURCES-MIB::hrStorageAllocationUnits"),
    ("1.3.6.1.2.1.25.2.3.1.5", "HOST-RESOURCES-MIB::hrStorageSize"),
    ("1.3.6.1.2.1.25.2.3.1.6", "HOST-RESOURCES-MIB::hrStorageUsed"),
    ("1.3.6.1.2.1.25.3.3.1.2", "HOST-RESOURCES-MIB::hrProcessorLoad"),
    // ENTITY-MIB
    ("1.3.6.1.2.1.47.1.1.1.1.2", "ENTITY-MIB::entPhysicalDescr"),
    ("1.3.6.1.2.1.47.1.1.1.1.7", "ENTITY-MIB::entPhysicalName"),
    ("1.3.6.1.2.1.47.1.1.1.1.11", "ENTITY-MIB::entPhysicalSerialNum"),
    ("1.3.6.1.2.1.47.1.1.1.1.13", "ENTITY-MIB::entPhysicalModelName"),
    // LLDP-MIB (IEEE 802.1AB)
    ("1.0.8802.1.1.2.1.3.3", "LLDP-MIB::lldpLocSysName"),
    ("1.0.8802.1.1.2.1.3.7.1.3", "LLDP-MIB::lldpLocPortId"),
    ("1.0.8802.1.1.2.1.4.1.1.5", "LLDP-MIB::lldpRemChassisId"),
    ("1.0.8802.1.1.2.1.4.1.1.7", "LLDP-MIB::lldpRemPortId"),
    ("1.0.8802.1.1.2.1.4.1.1.9", "LLDP-MIB::lldpRemSysName"),
    // CDP-MIB (Cisco)
    ("1.3.6.1.4.1.9.9.23.1.2.1.1.4", "CISCO-CDP-MIB::cdpCacheAddress"),
    ("1.3.6.1.4.1.9.9.23.1.2.1.1.6", "CISCO-CDP-MIB::cdpCacheDeviceId"),
    ("1.3.6.1.4.1.9.9.23.1.2.1.1.7", "CISCO-CDP-MIB::cdpCacheDevicePort"),
    ("1.3.6.1.4.1.9.9.23.1.2.1.1.8", "CISCO-CDP-MIB::cdpCachePlatform"),
    // Enterprise roots & well-known health OIDs
    ("1.3.6.1.4.1.9.9.109.1.1.1.1.8", "CISCO-PROCESS-MIB::cpmCPUTotal5minRev"),
    ("1.3.6.1.4.1.9.9.13.1.3.1.3", "CISCO-ENVMON-MIB::ciscoEnvMonTemperatureStatusValue"),
    ("1.3.6.1.4.1.14988.1.1.3.10", "MIKROTIK-MIB::mtxrHlTemperature"),
    ("1.3.6.1.4.1.14988.1.1.3.11", "MIKROTIK-MIB::mtxrHlProcessorTemperature"),
    ("1.3.6.1.4.1.14988.1.1.3.8", "MIKROTIK-MIB::mtxrHlVoltage"),
    ("1.3.6.1.4.1.2636.3.1.13.1.7", "JUNIPER-MIB::jnxOperatingTemp"),
    ("1.3.6.1.4.1.2636.3.1.13.1.8", "JUNIPER-MIB::jnxOperatingCPU"),
    ("1.3.6.1.4.1.2011.5.25.31.1.1.1.1.5", "HUAWEI-ENTITY-EXTENT-MIB::hwEntityCpuUsage"),
    ("1.3.6.1.4.1.6486.801.1.1.1.1.1.2.1.1", "ALCATEL-IND1-CHASSIS-MIB::chasEntTempCurrent"),
    ("1.3.6.1.4.1.14988", "Enterprises::MikroTik"),
    ("1.3.6.1.4.1.9", "Enterprises::Cisco"),
    ("1.3.6.1.4.1.2636", "Enterprises::Juniper"),
    ("1.3.6.1.4.1.2011", "Enterprises::Huawei"),
    ("1.3.6.1.4.1.3902", "Enterprises::ZTE"),
    ("1.3.6.1.4.1.4881", "Enterprises::Ruijie"),
    ("1.3.6.1.4.1.6486", "Enterprises::AlcatelLucent"),
    ("1.3.6.1.4.1.41112", "Enterprises::Ubiquiti"),
];

/// Resolves a numeric OID to the longest matching MIB symbol with instance suffix.
pub fn resolve_oid_name(oid: &str) -> String {
    let clean = oid.trim().trim_start_matches('.');
    let mut best: Option<(&str, &str)> = None;
    for &(prefix, name) in STANDARD_OIDS {
        if clean == prefix || clean.starts_with(&format!("{prefix}.")) {
            let replace = match best {
                None => true,
                Some((prev, _)) => prefix.len() > prev.len(),
            };
            if replace {
                best = Some((prefix, name));
            }
        }
    }
    match best {
        Some((prefix, name)) if clean.len() > prefix.len() => {
            let suffix = &clean[prefix.len()..];
            format!("{name}{suffix}")
        }
        Some((_, name)) => name.to_string(),
        None => clean.to_string(),
    }
}
