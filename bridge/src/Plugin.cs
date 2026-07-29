using BepInEx;
using Isto.GTW;
using UnityEngine;

namespace GTWSplitterBridge
{
    [BepInPlugin("com.gtw.splitterbridge", "GTW Splitter Bridge", "1.0.0")]
    public class Plugin : BaseUnityPlugin
    {
        private GTWProgressProvider _progress;
        private GTWGameState _gameState;

        // Previous values, so the log records transitions rather than every frame.
        private int _lastProgress = int.MinValue;
        private bool _lastPaused = true;
        private bool _lastEnded;
        private bool _loggedCheckpointNames;
        private bool _loggedReadError;

        private void Awake()
        {
            Logger.LogInfo("GTW Splitter Bridge loaded.");
        }

        private void Update()
        {
            if (_progress == null)
            {
                // Same lookup the game itself uses in GTWProgressProvider.Update.
                _progress = Object.FindAnyObjectByType<GTWProgressProvider>();
                if (_progress == null)
                {
                    if (GtwSplitterState.Attached)
                    {
                        GtwSplitterState.Attached = false;
                        _loggedCheckpointNames = false;
                        Logger.LogInfo("GTWProgressProvider went away; detached.");
                    }
                    return;
                }
            }

            if (_gameState == null)
            {
                _gameState = Object.FindAnyObjectByType<GTWGameState>();
            }

            try
            {
                int progress = _progress.GetCurrentGameProgressLevel();
                int max = _progress.GetMaxGameProgressLevel();

                GtwSplitterState.ProgressLevel = progress;
                GtwSplitterState.MaxProgressLevel = max;
                GtwSplitterState.GamePaused = _progress.GamePaused;
                GtwSplitterState.GameEnded = _progress.GameEnded;
                GtwSplitterState.TotalIgt = _progress.GetTotalGameSecondsElapsedInPlaythrough();
                GtwSplitterState.Mode = _gameState != null ? _gameState.SaveSlot : -1;

                LogCheckpointNamesOnce(max);
                LogTransitions(progress);

                if (!GtwSplitterState.Attached)
                {
                    GtwSplitterState.Attached = true;
                    Logger.LogInfo("Attached to GTWProgressProvider.");
                }

                _loggedReadError = false;
            }
            catch (System.Exception e)
            {
                GtwSplitterState.Attached = false;
                _loggedCheckpointNames = false;

                if (!_loggedReadError)
                {
                    _loggedReadError = true;
                    Logger.LogWarning("Read from GTWProgressProvider failed: "
                        + e.GetType().Name + ": " + e.Message);
                }
            }
        }

        /// <summary>
        /// Dumps the game's real speedrun checkpoint names so they can be
        /// reconciled against the segment list in the .lss file.
        /// </summary>
        private void LogCheckpointNamesOnce(int max)
        {
            if (_loggedCheckpointNames || max <= 0)
            {
                return;
            }

            _loggedCheckpointNames = true;
            Logger.LogInfo("MaxProgressLevel=" + max + ", Mode=" + GtwSplitterState.Mode);
            for (int i = 0; i < max; i++)
            {
                string name;
                try
                {
                    name = _progress.GetGameProgressLevelInternalName(i);
                }
                catch (System.Exception e)
                {
                    name = "<error: " + e.GetType().Name + ">";
                }

                Logger.LogInfo("  checkpoint[" + i + "] = " + name);
            }
        }

        private void LogTransitions(int progress)
        {
            if (progress != _lastProgress)
            {
                _lastProgress = progress;
                Logger.LogInfo("ProgressLevel -> " + progress
                    + " (igt=" + GtwSplitterState.TotalIgt.ToString("F2") + ")");
            }

            if (GtwSplitterState.GamePaused != _lastPaused)
            {
                _lastPaused = GtwSplitterState.GamePaused;
                Logger.LogInfo("GamePaused -> " + _lastPaused);
            }

            if (GtwSplitterState.GameEnded != _lastEnded)
            {
                _lastEnded = GtwSplitterState.GameEnded;
                Logger.LogInfo("GameEnded -> " + _lastEnded
                    + " (igt=" + GtwSplitterState.TotalIgt.ToString("F2") + ")");
            }
        }
    }
}
