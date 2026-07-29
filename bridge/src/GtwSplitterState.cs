namespace GTWSplitterBridge
{
    /// <summary>
    /// The entire contract between this mod and the wasm auto splitter.
    /// Field names and types are read by name from outside the process and
    /// must not be renamed or retyped without updating the autosplitter.
    /// </summary>
    public static class GtwSplitterState
    {
        /// <summary>True once a GTWProgressProvider has been located.</summary>
        public static bool Attached;

        /// <summary>GTWProgressProvider.GetCurrentGameProgressLevel(); -1 before the first checkpoint.</summary>
        public static int ProgressLevel = -1;

        /// <summary>GTWProgressProvider.GetMaxGameProgressLevel(); -1 until initialised.</summary>
        public static int MaxProgressLevel = -1;

        /// <summary>GTWProgressProvider.GamePaused; false after PLAYER_FIRST_INPUT.</summary>
        public static bool GamePaused = true;

        /// <summary>GTWProgressProvider.GameEnded; true after the EndOfGame event.</summary>
        public static bool GameEnded;

        /// <summary>GTWProgressProvider.GetTotalGameSecondsElapsedInPlaythrough(), already load- and pause-removed.</summary>
        public static float TotalIgt;

        /// <summary>GTWGameState.SaveSlot. 0 is the main game. Exposed for future category support; unused today.</summary>
        public static int Mode = -1;
    }
}
