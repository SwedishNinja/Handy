import React from "react";
import { useTranslation } from "react-i18next";
import { Slider } from "../../ui/Slider";
import { useSettings } from "../../../hooks/useSettings";

interface StreamingChunkDurationProps {
  descriptionMode?: "tooltip" | "inline";
  grouped?: boolean;
}

export const StreamingChunkDuration: React.FC<
  StreamingChunkDurationProps
> = ({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettings();

  const handleChange = (value: number) => {
    updateSetting(
      "streaming_chunk_duration_s",
      value === 0 ? null : value,
    );
  };

  return (
    <Slider
      value={settings?.streaming_chunk_duration_s ?? 0}
      onChange={handleChange}
      min={0}
      max={30}
      step={5}
      label={t("settings.debug.streamingChunkDuration.title")}
      description={t("settings.debug.streamingChunkDuration.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      formatValue={(v) =>
        v === 0 ? t("settings.debug.streamingChunkDuration.disabled") : `${v}s`
      }
    />
  );
};
