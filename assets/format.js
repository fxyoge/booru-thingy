export const size = (bytes) => bytes >= 1048576
    ? `${(bytes / 1048576).toFixed(1)} MB` : `${Math.ceil(bytes / 1024)} KB`;
