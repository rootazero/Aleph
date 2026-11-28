// The shape `PanelModel.view` returns (`src/lib.rs`).

export type PanelTab = {
  key: string;
  label: string;
  icon: string;
  status: "enabled" | "disabled" | "incompatible" | "failed";
  statusDetail: string | null;
  permissions: string[];
  renderer: "tsx" | "dom";
};

export type Slot = {
  key: string;
  state: "live" | "unavailable";
  renderer: "tsx" | "dom";
};

export type PanelView = {
  revision: number;
  tabs: PanelTab[];
  slots: Slot[];
  layout: { order: string[]; hidden: string[]; active: string | null };
  stored: string;
  repair: "corrupt" | "unknown_version" | null;
  projectionError: string | null;
};
