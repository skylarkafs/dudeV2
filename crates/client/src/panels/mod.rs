pub mod admin;
pub mod discovery_tools;
pub mod images;
pub mod inventory;
pub mod loader;
pub mod observability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveWorkspaceTab {
    Loader,
    Map,
    Devices,
    Services,
    Probes,
    DeviceTypes,
    Links,
    LinkTypes,
    Networks,
    Submaps,
    Outages,
    Events,
    Syslog,
    Traps,
    Charts,
    Discovery,
    Files,
    Tools,
    HistoryActions,
    ActiveAdmins,
    AdminsGroups,
    Notifications,
    ServerSettings,
    ClientSettings,
}
