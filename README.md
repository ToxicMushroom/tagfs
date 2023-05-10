# tagfs - tagging filesystem

Do you have thousands of scarce metadata files to search through (videos, images, non text formats) ?

By solving this problem with a file system we allow users to choose the file manager and thus the UI of interfacing with the tags.

## usage

Running the file system:
`tagfs --auto-unmount <mountpoint> <source_path>`

example fs root:
 - __all__ (default tag)
   - file1.mp4
   - file1.2.mp4
   - file2.1.mp4
   - file2.mp4
 - __tag1__
   - __tag2__
     - file1.2.mp4
   - file1.mp4
   - file1.2.mp4
 - __tag2__
   - __tag1__
     - file2.1.mp4
   - file2.1.mp4
   - file2.mp4

## roadmap
- [x] basic tagging
- [x] renaming tags
- [ ] remove tags
- [X] remove tag from file
- [X] save file
- [ ] recovery of a file's tags on rename/move/inode change | portability of the save file
- [ ] configuration for showing what tags and when (always show all tags even if empty, usefull for tagging new files)
- [ ] configuration for where to locate untagged files (fs root or maybe some default folder)
